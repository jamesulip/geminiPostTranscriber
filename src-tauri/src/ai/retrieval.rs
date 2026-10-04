//! Hybrid retrieval (tier 2): FTS5 for exact words (names, numbers, English terms) plus cosine
//! over chunk embeddings for meaning (Taglish ↔ English). Fused with reciprocal rank fusion.

use super::embed::{dot, embed, EmbedCall, EmbedTask};
use super::store::{self, StoredChunk};
use super::{AiCtx, Scope};
use crate::db;
use anyhow::Result;
use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Hit {
    pub chunk_id: i64,
    pub transcript_id: String,
    pub title: String,
    pub idx: i64,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub score: f64,
    /// "words" | "meaning" | "both"
    pub via: &'static str,
    /// The best-matching segment time for word hits (click target); else the chunk start.
    pub at: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Retrieval {
    pub hits: Vec<Hit>,
    /// Why meaning search was unavailable, if it was.
    pub note: Option<String>,
}

const RRF_K: f64 = 60.0;
const CANDIDATES: i64 = 60;

/// Top `k` chunks for `query` within `scope`.
pub async fn retrieve(ctx: &AiCtx<'_>, scope: &Scope, query: &str, k: usize) -> Result<Retrieval> {
    // Which transcripts, and which of them were never chunked (short lock).
    let (ids, titles, unchunked) = {
        let c = ctx.db();
        let ts = store::transcripts_in_scope(&c, scope)?;
        let ids: Vec<String> = ts.iter().map(|t| t.id.clone()).collect();
        let titles: HashMap<String, String> = ts.into_iter().map(|t| (t.id, t.title)).collect();
        let mut unchunked = Vec::new();
        for id in &ids {
            let n: i64 = c.query_row("SELECT COUNT(*) FROM ai_chunks WHERE transcript_id = ?1", [id], |r| r.get(0))?;
            if n == 0 {
                unchunked.push(id.clone());
            }
        }
        (ids, titles, unchunked)
    };
    // Transcripts never digested get chunks now (no model calls).
    for id in &unchunked {
        super::digest::ensure_chunks(ctx, id)?;
    }
    let (chunks, lexical) = {
        let c = ctx.db();
        (store::chunks_in(&c, &ids)?, fts_ranked(&c, query, &ids)?)
    };
    finish(ctx, query, k, titles, chunks, lexical).await
}

/// (transcript_id, segment idx, segment start) ranked by FTS5.
fn fts_ranked(c: &rusqlite::Connection, query: &str, ids: &[String]) -> Result<Vec<(String, i64, f64)>> {
    let Some(fq) = db::fts_query(query) else { return Ok(vec![]) };
    let mut st = c.prepare(
        "SELECT s.transcript_id, s.idx, s.start FROM segments_fts JOIN segments s ON s.id = segments_fts.rowid
         WHERE segments_fts MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows = st.query_map(rusqlite::params![fq, CANDIDATES * 3], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?)))?;
    let mut out = Vec::new();
    for r in rows {
        let r = r?;
        if ids.iter().any(|i| *i == r.0) {
            out.push(r);
        }
    }
    Ok(out)
}

async fn finish(ctx: &AiCtx<'_>, query: &str, k: usize, titles: HashMap<String, String>, chunks: Vec<StoredChunk>, lexical: Vec<(String, i64, f64)>) -> Result<Retrieval> {
    if chunks.is_empty() {
        return Ok(Retrieval::default());
    }
    // Lexical: segment hits → their chunk, first (best) hit per chunk keeps its time.
    let mut lex_rank: HashMap<i64, (usize, f64)> = HashMap::new();
    for (rank, (tid, idx, start)) in lexical.iter().enumerate() {
        if let Some(ch) = chunks.iter().find(|c| &c.transcript_id == tid && c.first_idx <= *idx && *idx <= c.last_idx) {
            lex_rank.entry(ch.id).or_insert((rank, *start));
        }
    }
    // Semantic: cosine against every chunk that has a vector for the configured model.
    let mut sem_rank: HashMap<i64, usize> = HashMap::new();
    let mut note = None;
    match ctx.embed_provider() {
        Ok(cfg) => {
            let with_vec: Vec<&StoredChunk> = chunks.iter().filter(|c| c.embedding.is_some() && c.embed_model.as_deref() == Some(cfg.model.as_str())).collect();
            if with_vec.is_empty() {
                note = Some("Not indexed for meaning search yet (summaries build the index).".into());
            } else {
                let on_retry = |_m: String| {};
                let call = EmbedCall { cancel: ctx.cancel, on_retry: &on_retry };
                match embed(ctx.http, &cfg, &[query.to_string()], EmbedTask::Query, &call).await {
                    Ok((q, _)) => {
                        let q = &q[0];
                        let mut scored: Vec<(i64, f32)> = with_vec.iter().map(|c| (c.id, dot(q, c.embedding.as_ref().unwrap()))).collect();
                        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
                        for (rank, (id, _)) in scored.into_iter().take(CANDIDATES as usize).enumerate() {
                            sem_rank.insert(id, rank);
                        }
                    }
                    Err(e) => note = Some(format!("Meaning search unavailable: {e:#}")),
                }
            }
        }
        Err(e) => note = Some(format!("Meaning search unavailable: {e:#}")),
    }
    // Fuse.
    let mut hits: Vec<Hit> = chunks
        .iter()
        .filter_map(|c| {
            let l = lex_rank.get(&c.id);
            let s = sem_rank.get(&c.id);
            if l.is_none() && s.is_none() {
                return None;
            }
            let score = l.map(|(r, _)| 1.0 / (RRF_K + *r as f64)).unwrap_or(0.0) + s.map(|r| 1.0 / (RRF_K + *r as f64)).unwrap_or(0.0);
            Some(Hit {
                chunk_id: c.id,
                transcript_id: c.transcript_id.clone(),
                title: titles.get(&c.transcript_id).cloned().unwrap_or_default(),
                idx: c.idx,
                start: c.start,
                end: c.end,
                text: c.text.clone(),
                score,
                via: match (l, s) {
                    (Some(_), Some(_)) => "both",
                    (Some(_), None) => "words",
                    _ => "meaning",
                },
                at: l.map(|(_, t)| *t).unwrap_or(c.start),
            })
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(k);
    Ok(Retrieval { hits, note })
}

#[cfg(test)]
mod tests {
    #[test]
    fn rrf_prefers_chunks_found_both_ways() {
        let both = 1.0 / (super::RRF_K + 5.0) + 1.0 / (super::RRF_K + 5.0);
        let top_lexical_only = 1.0 / super::RRF_K;
        assert!(both > top_lexical_only);
    }
}
