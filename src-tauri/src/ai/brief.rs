//! "Project brief": requirements, decisions, constraints and open questions for developers who
//! weren't in the meetings, built from stored digests (not transcripts) with citations.

use super::ask::validate_citations;
use super::chat::{complete, ChatCall, ChatMessage, ChatRequest, Role, SystemBlock};
use super::store;
use super::{estimate_tokens, format_time, prompts, usage, AiCtx, Scope};
use anyhow::{bail, Result};
use std::collections::HashMap;

/// Material per transcript is capped so a folder of many meetings still fits.
const PER_TRANSCRIPT_TOKENS: usize = 2500;

pub async fn brief(ctx: &AiCtx<'_>, scope: &Scope) -> Result<String> {
    let cfg = ctx.chat_provider()?;
    let key = "brief";
    let (material, allowed, titles, sources, missing) = {
        let c = ctx.db();
        let ts = store::transcripts_in_scope(&c, scope)?;
        if ts.is_empty() {
            bail!("No finished transcripts in this scope");
        }
        let mut material = String::new();
        let mut allowed: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
        let mut titles: HashMap<String, String> = HashMap::new();
        let mut sources = Vec::new();
        let mut missing = Vec::new();
        for t in &ts {
            let ds = store::digests_for(&c, &t.id)?;
            let get = |k: &str| ds.iter().find(|d| d.kind == k).map(|d| d.content.clone());
            let Some(summary) = get("summary") else {
                missing.push(t.title.clone());
                continue;
            };
            let mut block = format!("# {} (id {}, recorded {}, {})\n{}\n", t.title, t.id, t.recorded_at.as_deref().unwrap_or("?"), format_time(t.duration), summary);
            for k in ["decisions", "actions", "questions"] {
                if let Some(json) = get(k) {
                    if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                        if !items.is_empty() {
                            block.push_str(&format!("\n{k}:\n"));
                            for i in items {
                                let time = i["time"].as_f64().map(|s| format!(" [{}]", format_time(s))).unwrap_or_default();
                                let text = i["text"].as_str().map(str::to_string).unwrap_or_else(|| {
                                    format!("{}{}", i["owner"].as_str().map(|o| format!("{o}: ")).unwrap_or_default(), i["task"].as_str().unwrap_or(""))
                                });
                                block.push_str(&format!("- {text}{time}\n"));
                            }
                        }
                    }
                }
            }
            // Part summaries carry detail the digest may have dropped.
            for ch in store::chunks_for(&c, &t.id)? {
                if let Some(s) = ch.summary {
                    let line = format!("- [{}] {}\n", format_time(ch.start), s);
                    if estimate_tokens(&block) + estimate_tokens(&line) > PER_TRANSCRIPT_TOKENS {
                        break;
                    }
                    block.push_str(&line);
                }
            }
            material.push_str(&block);
            material.push('\n');
            allowed.insert(t.id.clone(), vec![(0.0, t.duration + 1.0)]);
            titles.insert(t.id.clone(), t.title.clone());
            sources.push(format!("- {} (id {})", t.title, t.id));
        }
        (material, allowed, titles, sources, missing)
    };
    if allowed.is_empty() {
        bail!("None of these transcripts has a summary yet. Summarise them first (Assistant → Summary).");
    }
    ctx.progress(key, "Writing the brief", 0.2);
    let system = [SystemBlock { text: prompts::BRIEF.into(), cache: false }];
    let msg = [ChatMessage { role: Role::User, content: format!("Meeting material:\n\n{material}") }];
    let req = ChatRequest { system: &system, messages: &msg, max_tokens: 8192, json: false, cached_content: None, temperature: Some(0.2) };
    let on_retry = |m: String| ctx.progress(key, m, 0.3);
    let call = ChatCall { cancel: ctx.cancel, on_retry: &on_retry, on_delta: None };
    let r = complete(ctx.http, &cfg, &req, &call).await?;
    let title_refs: HashMap<&str, &str> = titles.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (body, _) = validate_citations(&r.text, &allowed, &title_refs);
    let mut out = format!("{body}\n\n---\nSources:\n{}\n", sources.join("\n"));
    if !missing.is_empty() {
        out.push_str(&format!("\nNot included (no summary yet): {}\n", missing.join(", ")));
    }
    out.push_str(&format!("\nGenerated {} by {} from the Transcriber library. Citations are [[t:transcript id:seconds]].\n", chrono::Local::now().format("%Y-%m-%d"), cfg.model));
    {
        let c = ctx.db();
        if let Some(id) = scope.transcript_id() {
            if let Some(h) = store::transcript_hash(&c, id)? {
                store::put_digest(&c, id, "brief", &h, &cfg.model, &out)?;
            }
        }
        usage::record(&c, &usage::Record { kind: "ai", cfg: &cfg, usage: r.usage, requests: 1, status: "done", transcript_id: scope.transcript_id(), title: "Project brief" });
    }
    ctx.done(key, "Brief ready", None);
    Ok(out)
}
