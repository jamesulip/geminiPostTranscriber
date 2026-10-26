//! English translation of a transcript, chunk by chunk, stored once per content hash as the
//! `translation_en` digest: a JSON object of segment idx → English text.

use super::chat::{complete, ChatCall, ChatMessage, ChatRequest, Role, SystemBlock};
use super::store::{self, Digest};
use super::{digest::ensure_chunks, lines, prompts, usage, AiCtx};
use crate::providers::TokenUsage;
use anyhow::{anyhow, bail, Context, Result};
use futures::stream::{self, StreamExt};
use std::collections::BTreeMap;
use std::sync::Mutex;

const CONCURRENCY: usize = 3;

pub async fn translate_transcript(ctx: &AiCtx<'_>, id: &str, force: bool) -> Result<Digest> {
    let key = format!("translate:{id}");
    if !force {
        if let Some(d) = store::digest(&ctx.db(), id, "translation_en")? {
            if !d.stale {
                return Ok(d);
            }
        }
    }
    let cfg = ctx.chat_provider()?;
    let chunks = ensure_chunks(ctx, id)?;
    let hash = store::transcript_hash(&ctx.db(), id)?.ok_or_else(|| anyhow!("Transcript has no text yet"))?;
    let title: String = ctx.db().query_row("SELECT title FROM transcripts WHERE id = ?1", [id], |r| r.get(0))?;
    let n = chunks.len();
    if n == 0 {
        bail!("Transcript has no text yet");
    }
    ctx.progress(&key, format!("Translating {n} part(s)"), 0.02);
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
                let system = [SystemBlock { text: prompts::TRANSLATE.into(), cache: false }];
                let msg = [ChatMessage { role: Role::User, content: lines::with_ids(&ch.text, ch.first_idx) }];
                let req = ChatRequest { system: &system, messages: &msg, max_tokens: 8192, json: false, cached_content: None, temperature: Some(0.1) };
                let call = ChatCall { cancel: ctx.cancel, on_retry, on_delta: None };
                let r = complete(ctx.http, cfg, &req, &call).await.with_context(|| format!("translating part {}", ch.idx + 1))?;
                {
                    let mut t = total.lock().unwrap();
                    t.0 += r.usage;
                    t.1 += 1;
                }
                let map = lines::parse(&ch.text, &r.text, ch.first_idx);
                let mut d = done.lock().unwrap();
                *d += 1;
                ctx.progress(key, format!("Translated part {}/{n}", *d), 0.02 + 0.95 * (*d as f64 / n as f64));
                Ok(map)
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut all: BTreeMap<i64, String> = BTreeMap::new();
    let mut failed = None;
    for r in results {
        match r {
            Ok(m) => all.extend(m),
            Err(e) => failed = Some(e),
        }
    }
    let (u, requests) = *total.lock().unwrap();
    let status = if failed.is_some() { "error" } else { "done" };
    usage::record(&ctx.db(), &usage::Record { kind: "ai", cfg: &cfg, usage: u, requests, status, transcript_id: Some(id), title: &format!("Translation: {title}") });
    if let Some(e) = failed {
        ctx.done(&key, "", Some(format!("{e:#}")));
        return Err(e);
    }
    let content = serde_json::to_string(&all)?;
    store::put_digest(&ctx.db(), id, "translation_en", &hash, &cfg.model, &content)?;
    ctx.done(&key, "Translated", None);
    store::digest(&ctx.db(), id, "translation_en")?.ok_or_else(|| anyhow!("translation vanished"))
}
