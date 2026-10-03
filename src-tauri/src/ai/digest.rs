//! Per-transcript digests (tier 1 cache): summarise each chunk once (map), then one structured
//! digest per transcript (reduce). Everything is keyed by content hash so nothing is redone
//! unless the transcript changed. Also embeds chunks for semantic search.

use super::chat::{complete, ChatCall, ChatMessage, ChatRequest, Role, SystemBlock};
use super::chunker;
use super::embed::{embed, EmbedCall, EmbedTask};
use super::store::{self, Digest};
use super::{estimate_tokens, format_time, parse_json_lenient, prompts, usage, AiCtx};
use crate::db;
use crate::providers::TokenUsage;
use anyhow::{anyhow, bail, Context, Result};
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use std::sync::Mutex;

/// Chunk summaries run this many at a time (rate limits are retried anyway).
const MAP_CONCURRENCY: usize = 3;
/// Transcripts up to this size are digested from the full text instead of chunk summaries.
const FULL_TEXT_TOKENS: usize = 14_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DigestOutcome {
    pub transcript_id: String,
    pub digests: Vec<Digest>,
    /// Nothing to do: digests were current.
    pub skipped: bool,
    pub title_changed: bool,
}

pub const KINDS: &[&str] = &["summary", "decisions", "actions", "questions", "topics", "glossary", "speakers"];

struct Prepared {
    title: String,
    source_name: String,
    recorded_at: Option<String>,
    duration: f64,
    hash: String,
    chunks: Vec<store::StoredChunk>,
    speakers: Vec<String>,
}

/// Load the transcript, (re)compute its hash and sync its chunks. Short DB lock.
fn prepare(ctx: &AiCtx<'_>, id: &str) -> Result<Prepared> {
    let mut c = ctx.db();
    let t = db::get_transcript(&c, id)?.ok_or_else(|| anyhow!("Transcript not found"))?;
    if t.segments.is_empty() {
        bail!("This transcript has no text yet");
    }
    let hash = match t.summary.content_hash {
        Some(h) => h,
        None => db::refresh_content_hash(&c, id)?,
    };
    let names: HashMap<String, String> = t.speakers.iter().map(|s| (s.label.clone(), s.display_name.clone())).collect();
    let chunks = store::sync_chunks(&mut c, id, &chunker::chunk(&t.segments, &names))?;
    Ok(Prepared {
        title: t.summary.title,
        source_name: t.summary.source_name,
        recorded_at: t.summary.recorded_at,
        duration: t.summary.duration,
        hash,
        chunks,
        speakers: t.speakers.iter().map(|s| s.display_name.clone()).collect(),
    })
}

fn header(p: &Prepared) -> String {
    let mut h = format!("Meeting: {}\nRecording file: {}\nLength: {}\n", p.title, p.source_name, format_time(p.duration));
    if let Some(d) = &p.recorded_at {
        h.push_str(&format!("Recorded: {d}\n"));
    }
    if !p.speakers.is_empty() {
        h.push_str(&format!("Speakers: {}\n", p.speakers.join(", ")));
    }
    h
}

/// Summaries + structured digest for one transcript. `force` redoes everything.
pub async fn digest_transcript(ctx: &AiCtx<'_>, id: &str, force: bool) -> Result<DigestOutcome> {
    let key = format!("digest:{id}");
    ctx.progress(&key, "Preparing", 0.02);
    let p = prepare(ctx, id)?;
    if !force {
        let current = store::digest(&ctx.db(), id, "summary")?;
        if current.is_some_and(|d| !d.stale && d.prompt_version == prompts::PROMPT_VERSION) {
            ctx.done(&key, "Summary is up to date", None);
            return Ok(DigestOutcome { transcript_id: id.into(), digests: store::digests_for(&ctx.db(), id)?, skipped: true, title_changed: false });
        }
    }
    let cfg = ctx.chat_provider()?;
    let total = Mutex::new((TokenUsage::default(), 0i64));
    let on_retry = |m: String| ctx.progress(&key, m, 0.1);

    // Map: summarise chunks that have no summary yet (or all, when forced).
    let todo: Vec<store::StoredChunk> = p.chunks.iter().filter(|c| force || c.summary.is_none()).cloned().collect();
    let n = todo.len();
    let done_count = Mutex::new(0usize);
    let full_text_tokens: usize = p.chunks.iter().map(|c| estimate_tokens(&c.text)).sum();
    // Small transcripts go to the reducer whole; chunk summaries are still made for retrieval.
    if n > 0 {
        ctx.progress(&key, format!("Summarising {n} part(s)"), 0.05);
        let results: Vec<Result<()>> = stream::iter(todo)
            .map(|ch| {
                let cfg = &cfg;
                let total = &total;
                let done_count = &done_count;
                let key = &key;
                let on_retry = &on_retry;
                async move {
                    if ctx.is_cancelled() {
                        bail!("Cancelled");
                    }
                    let system = [SystemBlock { text: prompts::CHUNK_SUMMARY.into(), cache: false }];
                    let msg = [ChatMessage { role: Role::User, content: format!("Part {} ({}–{}):\n\n{}", ch.idx + 1, format_time(ch.start), format_time(ch.end), ch.text) }];
                    let req = ChatRequest { system: &system, messages: &msg, max_tokens: 1024, json: false, cached_content: None, temperature: Some(0.2) };
                    let call = ChatCall { cancel: ctx.cancel, on_retry, on_delta: None };
                    let r = complete(ctx.http, cfg, &req, &call).await.with_context(|| format!("summarising part {}", ch.idx + 1))?;
                    {
                        let mut t = total.lock().unwrap();
                        t.0 += r.usage;
                        t.1 += 1;
                    }
                    store::set_chunk_summary(&ctx.db(), ch.id, r.text.trim())?;
                    let mut d = done_count.lock().unwrap();
                    *d += 1;
                    ctx.progress(key, format!("Summarised part {}/{n}", *d), 0.05 + 0.6 * (*d as f64 / n as f64));
                    Ok(())
                }
            })
            .buffer_unordered(MAP_CONCURRENCY)
            .collect()
            .await;
        if let Some(e) = results.into_iter().find_map(Result::err) {
            record(ctx, &cfg, &total, "error", id, &p.title);
            ctx.done(&key, "", Some(format!("{e:#}")));
            return Err(e);
        }
    }

    // Reduce: one structured digest from the full text (short meetings) or the part summaries.
    ctx.progress(&key, "Writing the digest", 0.7);
    let chunks = store::chunks_for(&ctx.db(), id)?;
    let mut input = header(&p);
    if full_text_tokens <= FULL_TEXT_TOKENS {
        input.push_str("\nTranscript:\n");
        for ch in &chunks {
            input.push_str(&ch.text);
            input.push('\n');
        }
    } else {
        input.push_str("\nSummaries of each part:\n");
        for ch in &chunks {
            input.push_str(&format!("\n## Part {} [{}–{}]\n{}\n", ch.idx + 1, format_time(ch.start), format_time(ch.end), ch.summary.as_deref().unwrap_or("(no summary)")));
        }
    }
    let system = [SystemBlock { text: prompts::DIGEST.into(), cache: false }];
    let msg = [ChatMessage { role: Role::User, content: input }];
    let req = ChatRequest { system: &system, messages: &msg, max_tokens: 8192, json: true, cached_content: None, temperature: Some(0.2) };
    let call = ChatCall { cancel: ctx.cancel, on_retry: &on_retry, on_delta: None };
    let r = match complete(ctx.http, &cfg, &req, &call).await {
        Ok(r) => r,
        Err(e) => {
            record(ctx, &cfg, &total, "error", id, &p.title);
            ctx.done(&key, "", Some(format!("{e:#}")));
            return Err(e);
        }
    };
    {
        let mut t = total.lock().unwrap();
        t.0 += r.usage;
        t.1 += 1;
    }
    let v = parse_json_lenient(&r.text).context("reading the digest")?;
    let mut title_changed = false;
    {
        let c = ctx.db();
        let summary = v["summary"].as_str().unwrap_or("").trim().to_string();
        store::put_digest(&c, id, "summary", &p.hash, &cfg.model, &summary)?;
        for kind in ["decisions", "actions", "questions", "topics", "glossary", "speakers"] {
            let arr = v.get(kind).filter(|x| x.is_array()).cloned().unwrap_or_else(|| serde_json::json!([]));
            store::put_digest(&c, id, kind, &p.hash, &cfg.model, &arr.to_string())?;
        }
        // Name the meeting when it still carries the file name.
        let stem = std::path::Path::new(&p.source_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(t) = v["title"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
            if p.title == p.source_name || p.title == stem {
                c.execute("UPDATE transcripts SET title = ?2 WHERE id = ?1", rusqlite::params![id, t])?;
                title_changed = true;
            }
        }
    }
    record(ctx, &cfg, &total, "done", id, &p.title);

    // Embeddings for semantic search: best effort (needs an embeddings key).
    ctx.progress(&key, "Indexing for search", 0.9);
    if let Err(e) = embed_transcripts(ctx, &[id.to_string()], &key).await {
        eprintln!("embedding {id}: {e:#}");
    }
    ctx.done(&key, "Summary ready", None);
    Ok(DigestOutcome { transcript_id: id.into(), digests: store::digests_for(&ctx.db(), id)?, skipped: false, title_changed })
}

fn record(ctx: &AiCtx<'_>, cfg: &crate::settings::ProviderConfig, total: &Mutex<(TokenUsage, i64)>, status: &str, id: &str, title: &str) {
    let (u, requests) = *total.lock().unwrap();
    usage::record(&ctx.db(), &usage::Record { kind: "ai", cfg, usage: u, requests, status, transcript_id: Some(id), title: &format!("Summary: {title}") });
}

/// Embed every chunk (of the given transcripts) that lacks a vector for the configured model.
/// Returns how many chunks were embedded.
pub async fn embed_transcripts(ctx: &AiCtx<'_>, ids: &[String], key: &str) -> Result<usize> {
    let cfg = ctx.embed_provider()?;
    let missing = store::chunks_missing_embedding(&ctx.db(), ids, &cfg.model)?;
    if missing.is_empty() {
        return Ok(0);
    }
    let on_retry = |m: String| ctx.progress(key, m, 0.9);
    let call = EmbedCall { cancel: ctx.cancel, on_retry: &on_retry };
    let mut total = TokenUsage::default();
    let mut requests = 0;
    let mut n = 0;
    for batch in missing.chunks(32) {
        if ctx.is_cancelled() {
            bail!("Cancelled");
        }
        let texts: Vec<String> = batch.iter().map(|c| c.text.clone()).collect();
        let (vecs, u) = embed(ctx.http, &cfg, &texts, EmbedTask::Document, &call).await?;
        total += u;
        requests += 1;
        let c = ctx.db();
        for (ch, v) in batch.iter().zip(vecs) {
            store::set_chunk_embedding(&c, ch.id, &v, &cfg.model)?;
            n += 1;
        }
        ctx.progress(key, format!("Indexed {n}/{} part(s)", missing.len()), 0.9 + 0.1 * (n as f64 / missing.len() as f64));
    }
    usage::record(&ctx.db(), &usage::Record { kind: "embed", cfg: &cfg, usage: total, requests, status: "done", transcript_id: ids.first().map(String::as_str).filter(|_| ids.len() == 1), title: "Search index" });
    Ok(n)
}

/// Ensure chunks exist for a transcript (no model calls), e.g. before retrieval on a transcript
/// that was never digested.
pub fn ensure_chunks(ctx: &AiCtx<'_>, id: &str) -> Result<Vec<store::StoredChunk>> {
    let mut c = ctx.db();
    let existing = store::chunks_for(&c, id)?;
    let hash = store::transcript_hash(&c, id)?;
    // Chunks are in sync when their combined hash context matches; cheap check: any chunk + same hash.
    if !existing.is_empty() {
        let t_hash = hash.clone().unwrap_or_default();
        let digest_hash = store::digest(&c, id, "summary")?.map(|d| d.content_hash);
        if digest_hash.as_deref() == Some(t_hash.as_str()) {
            return Ok(existing);
        }
    }
    let t = db::get_transcript(&c, id)?.ok_or_else(|| anyhow!("Transcript not found"))?;
    if t.summary.content_hash.is_none() {
        db::refresh_content_hash(&c, id)?;
    }
    let names: HashMap<String, String> = t.speakers.iter().map(|s| (s.label.clone(), s.display_name.clone())).collect();
    store::sync_chunks(&mut c, id, &chunker::chunk(&t.segments, &names))
}
