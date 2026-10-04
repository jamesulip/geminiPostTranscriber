//! Questions over one transcript or many, with citations that seek the player.

use super::chat::{complete, ChatCall, ChatMessage, ChatRequest, Role, SystemBlock};
use super::retrieval::retrieve;
use super::store::{self, Citation, Message, NewMessage, Thread};
use super::{cache, digest, estimate_tokens, format_time, prompts, usage, AiCtx, AiEvent, Scope};
use crate::providers::TokenUsage;
use anyhow::{bail, Result};
use serde::Deserialize;
use sha2::Digest as _;
use std::collections::HashMap;

/// Whole transcripts up to this size go into the prompt as-is; bigger ones use retrieval.
pub const FULL_CONTEXT_TOKENS: usize = 20_000;
/// Budget for retrieved excerpts.
const EXCERPT_TOKENS: usize = 14_000;
const HISTORY_TURNS: usize = 12;
const DIGEST_CHARS: usize = 1500;

#[derive(Debug, Clone, Deserialize)]
pub struct AskParams {
    pub thread_id: Option<String>,
    pub scope: Scope,
    pub question: String,
    /// Put the whole transcript in context even when it's long (single transcript only).
    #[serde(default)]
    pub read_all: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Answer {
    pub thread: Thread,
    pub message: Message,
    pub note: Option<String>,
}

struct InScope {
    id: String,
    title: String,
    duration: f64,
    hash: String,
    summary: Option<String>,
}

pub async fn ask(ctx: &AiCtx<'_>, p: AskParams) -> Result<Answer> {
    let question = p.question.trim().to_string();
    if question.is_empty() {
        bail!("Ask something first");
    }
    let cfg = ctx.chat_provider()?;
    // Thread, history and what's in scope.
    let (thread, history, scope_ts) = {
        let c = ctx.db();
        let thread = match &p.thread_id {
            Some(id) => store::get_thread(&c, id)?.ok_or_else(|| anyhow::anyhow!("Conversation not found"))?,
            None => store::create_thread(&c, &title_from(&question), &p.scope)?,
        };
        let history = store::messages_for(&c, &thread.id)?;
        let ts = store::transcripts_in_scope(&c, &thread.scope)?;
        let mut scope_ts = Vec::new();
        for t in ts {
            let hash = match t.content_hash.clone() {
                Some(h) => h,
                None => crate::db::refresh_content_hash(&c, &t.id)?,
            };
            let summary = store::digest(&c, &t.id, "summary")?.map(|d| d.content);
            scope_ts.push(InScope { id: t.id, title: t.title, duration: t.duration, hash, summary });
        }
        (thread, history, scope_ts)
    };
    if scope_ts.is_empty() {
        bail!("There are no finished transcripts in this scope");
    }
    let scope = thread.scope.clone();
    let key = format!("ask:{}", thread.id);
    let emit = |delta: &str| (ctx.on_event)(AiEvent::Token { thread_id: thread.id.clone(), delta: delta.to_string() });

    // Record the question.
    let user_msg = NewMessage { thread_id: &thread.id, role: "user", content: &question, citations: &[], model: None, usage: TokenUsage::default(), cost_usd: 0.0 };
    store::add_message(&ctx.db(), &user_msg)?;

    // Memo: the same first question over unchanged transcripts is answered from the store.
    let first_turn = history.is_empty();
    let memo_key = first_turn.then(|| {
        let mut hashes: Vec<&str> = scope_ts.iter().map(|t| t.hash.as_str()).collect();
        hashes.sort();
        let mut h = sha2::Sha256::new();
        h.update(serde_json::to_string(&scope).unwrap_or_default());
        h.update(hashes.join(","));
        h.update(question.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" "));
        h.update(prompts::PROMPT_VERSION.to_le_bytes());
        h.update(&cfg.model);
        h.update([p.read_all as u8]);
        format!("{:x}", h.finalize())
    });
    if let Some(k) = &memo_key {
        // Bind first: a guard living in the `if let` scrutinee would still be held below (deadlock).
        let hit = store::memo_get(&ctx.db(), k)?;
        if let Some(prev) = hit {
            emit(&prev.content);
            let m = NewMessage { thread_id: &thread.id, role: "assistant", content: &prev.content, citations: &prev.citations, model: prev.model.as_deref(), usage: TokenUsage::default(), cost_usd: 0.0 };
            let mut saved = store::add_message(&ctx.db(), &m)?;
            saved.from_memo = true;
            ctx.done(&key, "Answered from an earlier identical question", None);
            return Ok(Answer { thread, message: saved, note: Some("Same question as before over unchanged transcripts: answered from the saved reply.".into()) });
        }
    }

    // Context: whole transcript (tier 3) or digests + retrieved excerpts (tiers 1 + 2).
    let mut system_text = String::from(prompts::ASK);
    system_text.push_str("\n\nTranscripts in scope (id → title):\n");
    for t in &scope_ts {
        system_text.push_str(&format!("- {} → {} ({})\n", t.id, t.title, format_time(t.duration)));
    }
    let mut context = String::new();
    let mut allowed: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    let mut cached_content: Option<String> = None;
    let mut note = None;
    let single = scope_ts.len() == 1;
    let mut full = false;
    if single {
        let t = &scope_ts[0];
        let chunks = digest::ensure_chunks(ctx, &t.id)?;
        let text: String = chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n");
        if p.read_all || estimate_tokens(&text) <= FULL_CONTEXT_TOKENS {
            full = true;
            ctx.progress(&key, "Reading the whole transcript", 0.1);
            context = format!("# {}\n\n{}", t.title, text);
            allowed.insert(t.id.clone(), vec![(0.0, t.duration + 1.0)]);
            if let Some(s) = &t.summary {
                context = format!("Summary:\n{s}\n\nFull transcript:\n{context}");
            }
            cached_content = cache::ensure_gemini_cache(ctx, &cfg, &t.id, &t.hash, &system_text, &context).await;
        }
    }
    if !full {
        ctx.progress(&key, "Finding the relevant parts", 0.1);
        let r = retrieve(ctx, &scope, &question, 10).await?;
        note = r.note.clone();
        for t in &scope_ts {
            if let Some(s) = &t.summary {
                let s: String = s.chars().take(DIGEST_CHARS).collect();
                context.push_str(&format!("## Summary of {} ({})\n{s}\n\n", t.title, t.id));
                allowed.entry(t.id.clone()).or_default().push((0.0, t.duration + 1.0));
            }
        }
        let mut used = estimate_tokens(&context);
        let mut any = false;
        for h in &r.hits {
            let block = format!("## Excerpt from {} ({}) [{}–{}]\n{}\n\n", h.title, h.transcript_id, format_time(h.start), format_time(h.end), h.text);
            let n = estimate_tokens(&block);
            if used + n > EXCERPT_TOKENS && any {
                break;
            }
            used += n;
            any = true;
            context.push_str(&block);
            allowed.entry(h.transcript_id.clone()).or_default().push((h.start - 30.0, h.end + 30.0));
        }
        if !any && scope_ts.iter().all(|t| t.summary.is_none()) {
            let text = "I couldn't find anything about that in these transcripts.";
            emit(text);
            let m = NewMessage { thread_id: &thread.id, role: "assistant", content: text, citations: &[], model: None, usage: TokenUsage::default(), cost_usd: 0.0 };
            let saved = store::add_message(&ctx.db(), &m)?;
            ctx.done(&key, "Nothing found", None);
            return Ok(Answer { thread, message: saved, note });
        }
    }

    // The prompt. With a Gemini cache, the system prompt and context live in the cache.
    let system: Vec<SystemBlock> = if cached_content.is_some() {
        vec![]
    } else {
        vec![SystemBlock { text: system_text.clone(), cache: false }, SystemBlock { text: format!("Transcript material:\n\n{context}"), cache: full }]
    };
    let mut messages: Vec<ChatMessage> = Vec::new();
    let start = history.len().saturating_sub(HISTORY_TURNS);
    for m in &history[start..] {
        messages.push(ChatMessage { role: if m.role == "user" { Role::User } else { Role::Assistant }, content: m.content.clone() });
    }
    // Keep roles alternating and the first message a user turn (Anthropic requires it).
    while messages.first().is_some_and(|m| m.role == Role::Assistant) {
        messages.remove(0);
    }
    messages.push(ChatMessage { role: Role::User, content: question.clone() });
    let req = ChatRequest { system: &system, messages: &messages, max_tokens: 4096, json: false, cached_content: cached_content.as_deref(), temperature: Some(0.3) };
    let on_retry = |m: String| ctx.progress(&key, m, 0.3);
    let on_delta = |d: &str| emit(d);
    let call = ChatCall { cancel: ctx.cancel, on_retry: &on_retry, on_delta: Some(&on_delta) };
    ctx.progress(&key, "Thinking", 0.3);
    let r = match complete(ctx.http, &cfg, &req, &call).await {
        Ok(r) => r,
        Err(e) => {
            ctx.done(&key, "", Some(format!("{e:#}")));
            return Err(e);
        }
    };
    let titles: HashMap<&str, &str> = scope_ts.iter().map(|t| (t.id.as_str(), t.title.as_str())).collect();
    let (content, citations) = validate_citations(&r.text, &allowed, &titles);
    let cost = usage::cost(&cfg, &r.usage);
    let saved = {
        let c = ctx.db();
        let m = NewMessage { thread_id: &thread.id, role: "assistant", content: &content, citations: &citations, model: Some(&cfg.model), usage: r.usage, cost_usd: cost };
        let saved = store::add_message(&c, &m)?;
        if let Some(k) = &memo_key {
            let _ = store::memo_put(&c, k, saved.id);
        }
        usage::record(&c, &usage::Record { kind: "ai", cfg: &cfg, usage: r.usage, requests: 1, status: "done", transcript_id: scope.transcript_id(), title: &format!("Q: {}", title_from(&question)) });
        saved
    };
    ctx.done(&key, "Answered", None);
    Ok(Answer { thread, message: saved, note })
}

/// Models sometimes group citations as `[[t:a:1], [t:b:2]]`; split them into separate tokens.
pub fn normalize_citations(text: &str) -> String {
    let group = regex::Regex::new(r"\[\[(t:[^\[\]]+(?:\]\s*,\s*\[t:[^\[\]]+)+)\]\]").unwrap();
    group
        .replace_all(text, |c: &regex::Captures| {
            c[1].split("],").map(|p| format!("[[{}]]", p.trim().trim_start_matches('[').trim_end_matches(']'))).collect::<Vec<_>>().join(" ")
        })
        .into_owned()
}

fn title_from(q: &str) -> String {
    let t: String = q.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= 60 {
        t
    } else {
        format!("{}…", t.chars().take(59).collect::<String>().trim_end())
    }
}

/// Keep `[[t:id:secs]]` tokens that point into the material the model was given; drop the rest.
pub fn validate_citations(text: &str, allowed: &HashMap<String, Vec<(f64, f64)>>, titles: &HashMap<&str, &str>) -> (String, Vec<Citation>) {
    let text = normalize_citations(text);
    let re = regex::Regex::new(r"\[\[t:([0-9A-Za-z_-]+):([0-9]+(?:\.[0-9]+)?)\]\]").unwrap();
    let mut cites: Vec<Citation> = Vec::new();
    let out = re.replace_all(&text, |caps: &regex::Captures| {
        let id = &caps[1];
        let secs: f64 = caps[2].parse().unwrap_or(-1.0);
        let ok = allowed.get(id).is_some_and(|ranges| ranges.iter().any(|(a, b)| secs >= *a && secs <= *b));
        if !ok {
            return String::new();
        }
        if !cites.iter().any(|c| c.transcript_id == id && (c.start - secs).abs() < 1.0) {
            cites.push(Citation { transcript_id: id.to_string(), title: titles.get(id).unwrap_or(&"").to_string(), start: secs });
        }
        caps[0].to_string()
    });
    (out.into_owned(), cites)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_citations_outside_the_material() {
        let allowed = HashMap::from([("abc".to_string(), vec![(100.0, 200.0)])]);
        let titles = HashMap::from([("abc", "Sept 13")]);
        let (text, cites) = validate_citations("Yes [[t:abc:150]] and no [[t:abc:999]] nor [[t:zzz:150]].", &allowed, &titles);
        assert_eq!(text, "Yes [[t:abc:150]] and no  nor .");
        assert_eq!(cites, vec![Citation { transcript_id: "abc".into(), title: "Sept 13".into(), start: 150.0 }]);
        assert_eq!(title_from("  what   was decided? "), "what was decided?");
        // Grouped form from real Gemini output.
        let allowed = HashMap::from([("abc".to_string(), vec![(0.0, 1000.0)])]);
        let (text, cites) = validate_citations("x [[t:abc:936], [t:abc:997]].", &allowed, &titles);
        assert_eq!(text, "x [[t:abc:936]] [[t:abc:997]].");
        assert_eq!(cites.len(), 2);
    }
}
