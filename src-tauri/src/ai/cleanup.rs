//! Non-verbatim copies of a transcript: fillers and false starts removed ("clean verbatim") or
//! turns rewritten into readable sentences ("smart"). Text-to-text from an existing version,
//! chunk by chunk, saved as a new version so the verbatim one stays in the history.

use super::chat::{complete, ChatCall, ChatMessage, ChatRequest, Role, SystemBlock};
use super::translate::align;
use super::{chunker, prompts, usage, AiCtx};
use crate::db::{self, SegmentRow, Speaker};
use crate::providers::{Segment, TokenUsage};
use anyhow::{anyhow, bail, Context, Result};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

const CONCURRENCY: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Remove fillers, stutters and false starts; keep every other word.
    Clean,
    /// Rewrite each turn into clear sentences with the same meaning.
    Smart,
}

impl Level {
    fn prompt(self) -> &'static str {
        match self {
            Level::Clean => prompts::CLEAN_VERBATIM,
            Level::Smart => prompts::SMART_REWRITE,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Level::Clean => "clean verbatim",
            Level::Smart => "smart rewrite",
        }
    }
}

/// Produce a cleaned copy of `version_id` (None = the current text) and commit it as the new
/// current version. Returns the new version number.
pub async fn clean_transcript(ctx: &AiCtx<'_>, id: &str, version_id: Option<i64>, level: Level) -> Result<i64> {
    let key = format!("cleanup:{id}");
    let cfg = ctx.chat_provider()?;
    // Source text, speakers and transcript facts (short lock).
    let (segments, speakers, language, processed, title) = {
        let c = ctx.db();
        let t = db::get_transcript(&c, id)?.ok_or_else(|| anyhow!("Transcript not found"))?;
        let (segments, speakers): (Vec<SegmentRow>, Vec<Speaker>) = match version_id {
            Some(v) => {
                let v = db::get_version(&c, v)?.ok_or_else(|| anyhow!("Version not found"))?;
                (v.segments, v.speakers)
            }
            None => (t.segments, t.speakers),
        };
        (segments, speakers, t.summary.language, t.summary.processed_duration, t.summary.title)
    };
    if segments.is_empty() {
        bail!("Nothing to clean up: that version has no text");
    }
    let names: HashMap<String, String> = speakers.iter().map(|s| (s.label.clone(), s.display_name.clone())).collect();
    let chunks = chunker::chunk(&segments, &names);
    let n = chunks.len();
    ctx.progress(&key, format!("Cleaning up {n} part(s)"), 0.02);
    let total = Mutex::new((TokenUsage::default(), 0i64));
    let done = Mutex::new(0usize);
    let on_retry = |m: String| ctx.progress(&key, m, 0.1);
    let results: Vec<Result<BTreeMap<i64, String>>> = stream::iter(chunks)
        .map(|ch| {
            let (cfg, total, done, key, on_retry) = (&cfg, &total, &done, &key, &on_retry);
            async move {
                if ctx.is_cancelled() {
                    bail!("Cancelled");
                }
                let system = [SystemBlock { text: level.prompt().into(), cache: false }];
                let msg = [ChatMessage { role: Role::User, content: ch.text.clone() }];
                let req = ChatRequest { system: &system, messages: &msg, max_tokens: 8192, json: false, cached_content: None, temperature: Some(0.1) };
                let call = ChatCall { cancel: ctx.cancel, on_retry, on_delta: None };
                let r = complete(ctx.http, cfg, &req, &call).await.with_context(|| format!("cleaning part {}", ch.idx + 1))?;
                {
                    let mut t = total.lock().unwrap();
                    t.0 += r.usage;
                    t.1 += 1;
                }
                let map = align(&ch.text, &r.text, ch.first_idx);
                let mut d = done.lock().unwrap();
                *d += 1;
                ctx.progress(key, format!("Cleaned part {}/{n}", *d), 0.02 + 0.93 * (*d as f64 / n as f64));
                Ok(map)
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut cleaned: BTreeMap<i64, String> = BTreeMap::new();
    let mut failed = None;
    for r in results {
        match r {
            Ok(m) => cleaned.extend(m),
            Err(e) => failed = Some(e),
        }
    }
    let (u, requests) = *total.lock().unwrap();
    let status = if failed.is_some() { "error" } else { "done" };
    usage::record(&ctx.db(), &usage::Record { kind: "ai", cfg: &cfg, usage: u, requests, status, transcript_id: Some(id), title: &format!("Clean-up ({}): {title}", level.label()) });
    if let Some(e) = failed {
        ctx.done(&key, "", Some(format!("{e:#}")));
        return Err(e);
    }
    // Lines the model dropped (filler-only turns) go; lines it skipped keep their original text.
    let segs: Vec<Segment> = segments
        .iter()
        .filter_map(|s| {
            let text = match cleaned.get(&s.idx) {
                Some(t) if t.trim().is_empty() || t.trim() == "-" || t.trim() == "—" => return None,
                Some(t) => t.trim().to_string(),
                None => s.text.clone(),
            };
            Some(Segment { start: s.start, end: s.end, text, speaker: s.speaker.clone() })
        })
        .collect();
    if segs.len() < segments.len() / 2 {
        ctx.done(&key, "", Some("The model dropped more than half of the lines; nothing was saved.".into()));
        bail!("The model dropped more than half of the lines; nothing was saved");
    }
    let number = {
        let mut c = ctx.db();
        let model = format!("{} · {}", cfg.model, level.label());
        let v = db::CompletedVersion { provider: "ai", model: &model, language: language.as_deref(), detected_language: None, processed_duration: processed, notes: None };
        db::commit_version_with_speakers(&mut c, id, &segs, &v, &speakers)?;
        c.query_row("SELECT version FROM transcripts WHERE id = ?1", [id], |r| r.get::<_, i64>(0))?
    };
    ctx.done(&key, format!("Saved as v{number} ({}); the verbatim text is in the history", level.label()), None);
    Ok(number)
}
