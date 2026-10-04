//! SQL for the `ai_*` tables.

use super::chunker::Chunk;
use super::embed::{from_blob, to_blob};
use super::Scope;
use crate::db::{self, TranscriptSummary};
use crate::providers::TokenUsage;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct StoredChunk {
    pub id: i64,
    pub transcript_id: String,
    pub idx: i64,
    pub start: f64,
    pub end: f64,
    pub first_idx: i64,
    pub last_idx: i64,
    pub chunk_hash: String,
    pub text: String,
    pub summary: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub embed_model: Option<String>,
}

fn chunk_from_row(r: &rusqlite::Row) -> rusqlite::Result<StoredChunk> {
    let blob: Option<Vec<u8>> = r.get(10)?;
    Ok(StoredChunk {
        id: r.get(0)?,
        transcript_id: r.get(1)?,
        idx: r.get(2)?,
        start: r.get(3)?,
        end: r.get(4)?,
        first_idx: r.get(5)?,
        last_idx: r.get(6)?,
        chunk_hash: r.get(7)?,
        text: r.get(8)?,
        summary: r.get(9)?,
        embedding: blob.map(|b| from_blob(&b)),
        embed_model: r.get(11)?,
    })
}

const CHUNK_COLS: &str = "id, transcript_id, idx, start, end, first_idx, last_idx, chunk_hash, text, summary, embedding, embed_model";

/// Replace a transcript's chunks, keeping summaries/embeddings of chunks whose hash is unchanged.
pub fn sync_chunks(c: &mut Connection, transcript_id: &str, chunks: &[Chunk]) -> Result<Vec<StoredChunk>> {
    let tx = c.transaction()?;
    let mut keep: HashMap<String, (Option<String>, Option<Vec<u8>>, Option<String>)> = HashMap::new();
    {
        let mut st = tx.prepare("SELECT chunk_hash, summary, embedding, embed_model FROM ai_chunks WHERE transcript_id = ?1")?;
        for row in st.query_map([transcript_id], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))? {
            let (h, v) = row?;
            keep.insert(h, v);
        }
    }
    tx.execute("DELETE FROM ai_chunks WHERE transcript_id = ?1", [transcript_id])?;
    {
        let mut st = tx.prepare(
            "INSERT INTO ai_chunks (transcript_id, idx, start, end, first_idx, last_idx, chunk_hash, text, summary, embedding, embed_model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        for ch in chunks {
            let (summary, embedding, model) = keep.get(&ch.hash).cloned().unwrap_or_default();
            st.execute(params![transcript_id, ch.idx, ch.start, ch.end, ch.first_idx, ch.last_idx, ch.hash, ch.text, summary, embedding, model])?;
        }
    }
    tx.commit()?;
    chunks_for(c, transcript_id)
}

pub fn chunks_for(c: &Connection, transcript_id: &str) -> Result<Vec<StoredChunk>> {
    let mut st = c.prepare(&format!("SELECT {CHUNK_COLS} FROM ai_chunks WHERE transcript_id = ?1 ORDER BY idx"))?;
    let rows: Vec<StoredChunk> = st.query_map([transcript_id], chunk_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn chunks_in(c: &Connection, ids: &[String]) -> Result<Vec<StoredChunk>> {
    let mut out = Vec::new();
    for id in ids {
        out.extend(chunks_for(c, id)?);
    }
    Ok(out)
}

pub fn set_chunk_summary(c: &Connection, id: i64, summary: &str) -> Result<()> {
    c.execute("UPDATE ai_chunks SET summary = ?2 WHERE id = ?1", params![id, summary])?;
    Ok(())
}

pub fn set_chunk_embedding(c: &Connection, id: i64, v: &[f32], model: &str) -> Result<()> {
    c.execute("UPDATE ai_chunks SET embedding = ?2, embed_model = ?3 WHERE id = ?1", params![id, to_blob(v), model])?;
    Ok(())
}

/// Transcript ids (in scope) that have chunks without an embedding for `model`.
pub fn chunks_missing_embedding(c: &Connection, ids: &[String], model: &str) -> Result<Vec<StoredChunk>> {
    Ok(chunks_in(c, ids)?.into_iter().filter(|ch| ch.embedding.is_none() || ch.embed_model.as_deref() != Some(model)).collect())
}

// Digests ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Digest {
    pub kind: String,
    pub content_hash: String,
    pub prompt_version: i64,
    pub model: String,
    /// Markdown (summary, brief, translation) or JSON (lists).
    pub content: String,
    pub created_at: String,
    /// The transcript changed since this was made.
    pub stale: bool,
}

pub fn digests_for(c: &Connection, transcript_id: &str) -> Result<Vec<Digest>> {
    let current: Option<String> = transcript_hash(c, transcript_id)?;
    let mut st = c.prepare(
        "SELECT kind, content_hash, prompt_version, model, content, created_at FROM ai_digests WHERE transcript_id = ?1 ORDER BY kind",
    )?;
    let rows = st.query_map([transcript_id], |r| {
        let hash: String = r.get(1)?;
        Ok(Digest {
            kind: r.get(0)?,
            stale: current.as_deref() != Some(hash.as_str()),
            content_hash: hash,
            prompt_version: r.get(2)?,
            model: r.get(3)?,
            content: r.get(4)?,
            created_at: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn digest(c: &Connection, transcript_id: &str, kind: &str) -> Result<Option<Digest>> {
    Ok(digests_for(c, transcript_id)?.into_iter().find(|d| d.kind == kind))
}

pub fn put_digest(c: &Connection, transcript_id: &str, kind: &str, hash: &str, model: &str, content: &str) -> Result<()> {
    c.execute(
        "INSERT INTO ai_digests (transcript_id, kind, content_hash, prompt_version, model, content, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(transcript_id, kind) DO UPDATE SET content_hash = excluded.content_hash,
           prompt_version = excluded.prompt_version, model = excluded.model, content = excluded.content,
           created_at = excluded.created_at",
        params![transcript_id, kind, hash, super::prompts::PROMPT_VERSION, model, content, chrono::Local::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn transcript_hash(c: &Connection, id: &str) -> Result<Option<String>> {
    Ok(c.query_row("SELECT content_hash FROM transcripts WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten())
}

/// Transcripts a scope covers (done ones only).
pub fn transcripts_in_scope(c: &Connection, scope: &Scope) -> Result<Vec<TranscriptSummary>> {
    let filter = match scope {
        Scope::Transcript { .. } | Scope::Library => db::ListFilter::default(),
        Scope::Folder { id } => db::ListFilter { folder_id: Some(*id), tag_id: None, recursive: true, trashed: false },
        Scope::Tag { id } => db::ListFilter { folder_id: None, tag_id: Some(*id), recursive: false, trashed: false },
    };
    let mut all = db::list_transcripts(c, &filter)?;
    if let Scope::Transcript { id } = scope {
        all.retain(|t| &t.id == id);
    }
    all.retain(|t| t.status == "done" || t.status == "queued");
    Ok(all)
}

// Threads ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Thread {
    pub id: String,
    pub title: String,
    pub scope: Scope,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Citation {
    pub transcript_id: String,
    pub title: String,
    pub start: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: i64,
    pub thread_id: String,
    pub role: String,
    pub content: String,
    pub citations: Vec<Citation>,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub created_at: String,
    /// Served from the answer memo (no model call).
    #[serde(default)]
    pub from_memo: bool,
}

fn thread_from_row(r: &rusqlite::Row) -> rusqlite::Result<Thread> {
    let scope: String = r.get(2)?;
    Ok(Thread {
        id: r.get(0)?,
        title: r.get(1)?,
        scope: serde_json::from_str(&scope).unwrap_or(Scope::Library),
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
    })
}

pub fn create_thread(c: &Connection, title: &str, scope: &Scope) -> Result<Thread> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Local::now().to_rfc3339();
    c.execute(
        "INSERT INTO ai_threads (id, title, scope_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, title, serde_json::to_string(scope)?, now],
    )?;
    Ok(Thread { id, title: title.to_string(), scope: scope.clone(), created_at: now.clone(), updated_at: now })
}

pub fn get_thread(c: &Connection, id: &str) -> Result<Option<Thread>> {
    Ok(c.query_row("SELECT id, title, scope_json, created_at, updated_at FROM ai_threads WHERE id = ?1", [id], thread_from_row).optional()?)
}

/// Threads, newest first; `scope` filters to threads with exactly that scope.
pub fn list_threads(c: &Connection, scope: Option<&Scope>) -> Result<Vec<Thread>> {
    let mut st = c.prepare("SELECT id, title, scope_json, created_at, updated_at FROM ai_threads ORDER BY updated_at DESC LIMIT 200")?;
    let mut out: Vec<Thread> = st.query_map([], thread_from_row)?.collect::<Result<_, _>>()?;
    if let Some(s) = scope {
        out.retain(|t| &t.scope == s);
    }
    Ok(out)
}

pub fn touch_thread(c: &Connection, id: &str) -> Result<()> {
    c.execute("UPDATE ai_threads SET updated_at = ?2 WHERE id = ?1", params![id, chrono::Local::now().to_rfc3339()])?;
    Ok(())
}

pub fn delete_thread(c: &Connection, id: &str) -> Result<()> {
    c.execute("DELETE FROM ai_threads WHERE id = ?1", [id])?;
    Ok(())
}

fn message_from_row(r: &rusqlite::Row) -> rusqlite::Result<Message> {
    let cites: Option<String> = r.get(4)?;
    Ok(Message {
        id: r.get(0)?,
        thread_id: r.get(1)?,
        role: r.get(2)?,
        content: r.get(3)?,
        citations: cites.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        model: r.get(5)?,
        input_tokens: r.get(6)?,
        cached_tokens: r.get(7)?,
        output_tokens: r.get(8)?,
        cost_usd: r.get(9)?,
        created_at: r.get(10)?,
        from_memo: false,
    })
}

const MSG_COLS: &str = "id, thread_id, role, content, citations_json, model, input_tokens, cached_tokens, output_tokens, cost_usd, created_at";

pub fn messages_for(c: &Connection, thread_id: &str) -> Result<Vec<Message>> {
    let mut st = c.prepare(&format!("SELECT {MSG_COLS} FROM ai_messages WHERE thread_id = ?1 ORDER BY id"))?;
    let rows: Vec<Message> = st.query_map([thread_id], message_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn get_message(c: &Connection, id: i64) -> Result<Option<Message>> {
    Ok(c.query_row(&format!("SELECT {MSG_COLS} FROM ai_messages WHERE id = ?1"), [id], message_from_row).optional()?)
}

pub struct NewMessage<'a> {
    pub thread_id: &'a str,
    pub role: &'a str,
    pub content: &'a str,
    pub citations: &'a [Citation],
    pub model: Option<&'a str>,
    pub usage: TokenUsage,
    pub cost_usd: f64,
}

pub fn add_message(c: &Connection, m: &NewMessage) -> Result<Message> {
    c.execute(
        "INSERT INTO ai_messages (thread_id, role, content, citations_json, model, input_tokens, cached_tokens, output_tokens, cost_usd, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            m.thread_id, m.role, m.content, serde_json::to_string(m.citations)?, m.model,
            m.usage.input_tokens as i64, m.usage.cached_tokens as i64, m.usage.output_tokens as i64, m.cost_usd,
            chrono::Local::now().to_rfc3339()
        ],
    )?;
    touch_thread(c, m.thread_id)?;
    Ok(get_message(c, c.last_insert_rowid())?.expect("just inserted"))
}

// Memo & provider caches ----------------------------------------------------------------------

pub fn memo_get(c: &Connection, key: &str) -> Result<Option<Message>> {
    let id: Option<i64> = c.query_row("SELECT message_id FROM ai_answer_memo WHERE key = ?1", [key], |r| r.get(0)).optional()?;
    match id {
        Some(id) => get_message(c, id),
        None => Ok(None),
    }
}

pub fn memo_put(c: &Connection, key: &str, message_id: i64) -> Result<()> {
    c.execute("INSERT OR REPLACE INTO ai_answer_memo (key, message_id) VALUES (?1, ?2)", params![key, message_id])?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ContextCache {
    pub cache_name: String,
    pub content_hash: String,
    pub expires_at: String,
    pub tokens: i64,
}

pub fn context_cache_get(c: &Connection, transcript_id: &str, model: &str) -> Result<Option<ContextCache>> {
    Ok(c
        .query_row(
            "SELECT cache_name, content_hash, expires_at, tokens FROM ai_context_cache WHERE transcript_id = ?1 AND model = ?2",
            params![transcript_id, model],
            |r| Ok(ContextCache { cache_name: r.get(0)?, content_hash: r.get(1)?, expires_at: r.get(2)?, tokens: r.get(3)? }),
        )
        .optional()?)
}

pub fn context_cache_put(c: &Connection, transcript_id: &str, model: &str, cc: &ContextCache) -> Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO ai_context_cache (transcript_id, model, content_hash, cache_name, expires_at, tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![transcript_id, model, cc.content_hash, cc.cache_name, cc.expires_at, cc.tokens],
    )?;
    Ok(())
}

pub fn context_cache_delete(c: &Connection, transcript_id: &str, model: &str) -> Result<()> {
    c.execute("DELETE FROM ai_context_cache WHERE transcript_id = ?1 AND model = ?2", params![transcript_id, model])?;
    Ok(())
}

// Glossary ---------------------------------------------------------------------------------

/// Terms (names, products, jargon) from the AI glossaries of transcripts in `folder_id`
/// (sub-folders included), or the whole library when None. Most frequent first.
pub fn glossary_terms(c: &Connection, folder_id: Option<i64>, limit: usize) -> Result<Vec<String>> {
    let filter = db::ListFilter { folder_id, tag_id: None, recursive: true, trashed: false };
    let ts = db::list_transcripts(c, &filter)?;
    let mut counts: HashMap<String, (usize, String)> = HashMap::new();
    for t in &ts {
        let Some(d) = digest(c, &t.id, "glossary")? else { continue };
        let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&d.content) else { continue };
        for term in items.iter().filter_map(|i| i["term"].as_str()).map(str::trim).filter(|t| !t.is_empty() && t.len() <= 40) {
            let e = counts.entry(term.to_lowercase()).or_insert((0, term.to_string()));
            e.0 += 1;
        }
    }
    let mut v: Vec<(usize, String)> = counts.into_values().collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Ok(v.into_iter().take(limit).map(|(_, t)| t).collect())
}

/// Glossary for a transcription job: the transcript's folder, else the whole library. Never fails.
pub fn glossary_for_job(c: &Connection, transcript_id: &str) -> Vec<String> {
    let folder: Option<i64> = c.query_row("SELECT folder_id FROM transcripts WHERE id = ?1", [transcript_id], |r| r.get(0)).ok().flatten();
    glossary_terms(c, folder, 40).unwrap_or_default()
}
