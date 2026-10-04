//! Documents generated from meetings: minutes, action-item emails, status updates, briefs,
//! user stories… Built from stored digests and part summaries (not whole transcripts), with
//! citations, from a built-in or user-defined template.

use super::ask::validate_citations;
use super::chat::{complete, ChatCall, ChatMessage, ChatRequest, Role, SystemBlock};
use super::store;
use super::{estimate_tokens, format_time, usage, AiCtx, Scope};
use crate::settings::Settings;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Material per transcript is capped so a folder of many meetings still fits.
const PER_TRANSCRIPT_TOKENS: usize = 2500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: String,
    /// What to produce; the harness adds the citation/language rules and the material.
    pub prompt: String,
    #[serde(default)]
    pub builtin: bool,
}

const RULES: &str = "\n\nRules: write in English (keep names, product names and technical terms as spoken; the material may mix \
Filipino/Tagalog and English). Use only what the material supports; never invent attendees, dates, numbers or \
commitments. After each statement of fact put citation tokens [[t:TRANSCRIPT_ID:SECONDS]] using transcript ids and \
timestamps from the material, each as its own token. Output markdown only, no preamble.";

pub fn builtin_templates() -> Vec<Template> {
    let t = |id: &str, name: &str, description: &str, prompt: &str| Template { id: id.into(), name: name.into(), description: description.into(), prompt: prompt.into(), builtin: true };
    vec![
        t("mom", "Minutes of meeting", "Formal minutes: attendees, agenda, discussion, decisions, action items, next meeting.",
          "Write formal minutes of the meeting(s). Structure:\n# Minutes — <meeting title(s)>\n**Date:** … **Duration:** … **Attendees:** (speaker names from the material; use the labels given if names are unknown)\n\n## Agenda\nnumbered list of topics in order\n\n## Discussion\none short subsection per agenda item: what was presented, main points, concerns raised\n\n## Decisions\nbullets\n\n## Action items\na markdown table: Owner | Task | Due | Status\n\n## Open items / parked\nbullets\n\n## Next meeting\nif mentioned"),
        t("brief", "Project brief", "Requirements, decisions, constraints, open questions and next steps for developers who weren't there.",
          "Write a project brief for software developers who did not attend. Sections, each a bullet list: ## Context, ## Requirements, ## Decisions, ## Constraints & risks, ## Open questions, ## Next steps. Leave a section as \"- none recorded\" if empty."),
        t("actions", "Action items email", "A short email listing who committed to what, grouped by owner, with due dates.",
          "Write a short follow-up email (subject line first) to the attendees listing the action items grouped by owner, with due dates where given, then the decisions in one short list. Friendly but concise; no filler."),
        t("status", "Status update", "Stakeholder update: progress, decisions, risks, decisions needed, next steps.",
          "Write a stakeholder status update: ## Progress (what moved forward), ## Decisions made, ## Risks & blockers, ## Decisions needed from stakeholders, ## Next steps with owners. 150–300 words plus bullets."),
        t("decisions", "Decision log", "Every decision with its rationale, who made it and when.",
          "Produce a decision log as a markdown table: Decision | Rationale | Made by | When (time in meeting) | Follow-ups. One row per decision, most consequential first. Then a short ## Reversed or deferred list if any."),
        t("stories", "User stories", "Requirements as user stories with acceptance criteria, ready for the backlog.",
          "Turn the requirements discussed into user stories for the backlog. For each: ### <short title>\n**As a** <role> **I want** <capability> **so that** <benefit>.\n**Acceptance criteria:** bullet list (Given/When/Then where natural).\n**Notes:** constraints, open questions. Group related stories under ## headings by module/feature."),
        t("tasks", "Dev task list", "Ticket-ready engineering tasks with descriptions and acceptance criteria.",
          "List the engineering tasks implied by the meeting as tickets. For each: ### <imperative title>\n**Description:** 1–3 sentences.\n**Acceptance criteria:** bullets.\n**Owner / due:** if mentioned.\n**Depends on:** other tasks if clear. Order by priority as discussed; mark anything still undecided with *(needs decision)*."),
        t("faq", "Questions & answers", "Questions raised in the meeting and the answers given (or marked unanswered).",
          "List the questions raised during the meeting(s) as a Q&A. For each: **Q:** the question (who asked, if known) then **A:** the answer given, or *Unanswered* with what would resolve it. Group by topic."),
    ]
}

/// Built-in templates plus the user's own from Settings (custom ones can override an id).
pub fn templates(settings: &Settings) -> Vec<Template> {
    let mut all = builtin_templates();
    for c in &settings.ai_templates {
        if c.prompt.trim().is_empty() || c.name.trim().is_empty() {
            continue;
        }
        let t = Template { id: c.id.clone(), name: c.name.clone(), description: c.description.clone(), prompt: c.prompt.clone(), builtin: false };
        match all.iter_mut().find(|x| x.id == t.id) {
            Some(x) => *x = t,
            None => all.push(t),
        }
    }
    all
}

struct Material {
    text: String,
    allowed: HashMap<String, Vec<(f64, f64)>>,
    titles: HashMap<String, String>,
    sources: Vec<String>,
    missing: Vec<String>,
}

fn material(ctx: &AiCtx<'_>, scope: &Scope) -> Result<Material> {
    let c = ctx.db();
    let ts = store::transcripts_in_scope(&c, scope)?;
    if ts.is_empty() {
        bail!("No finished transcripts in this scope");
    }
    let mut m = Material { text: String::new(), allowed: HashMap::new(), titles: HashMap::new(), sources: Vec::new(), missing: Vec::new() };
    for t in &ts {
        let ds = store::digests_for(&c, &t.id)?;
        let get = |k: &str| ds.iter().find(|d| d.kind == k).map(|d| d.content.clone());
        let Some(summary) = get("summary") else {
            m.missing.push(t.title.clone());
            continue;
        };
        let speakers: Vec<String> = crate::db::speakers_for(&c, &t.id)?.into_iter().map(|s| s.display_name).collect();
        let mut block = format!(
            "# {} (id {})\nRecorded: {}\nDuration: {}\nSpeakers: {}\n\n{}\n",
            t.title, t.id, t.recorded_at.as_deref().unwrap_or("unknown"), format_time(t.duration),
            if speakers.is_empty() { "unknown".to_string() } else { speakers.join(", ") }, summary
        );
        for k in ["decisions", "actions", "questions", "topics"] {
            if let Some(json) = get(k) {
                if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                    if !items.is_empty() {
                        block.push_str(&format!("\n{k}:\n"));
                        for i in items {
                            let time = i["time"].as_f64().or(i["start"].as_f64()).map(|s| format!(" [{}]", format_time(s))).unwrap_or_default();
                            let text = i["text"].as_str().or(i["title"].as_str()).map(str::to_string).unwrap_or_else(|| {
                                format!("{}{}{}", i["owner"].as_str().map(|o| format!("{o}: ")).unwrap_or_default(), i["task"].as_str().unwrap_or(""), i["due"].as_str().map(|d| format!(" (due {d})")).unwrap_or_default())
                            });
                            block.push_str(&format!("- {text}{time}\n"));
                        }
                    }
                }
            }
        }
        // Part summaries carry detail the digest may have dropped.
        let mut parts = String::new();
        for ch in store::chunks_for(&c, &t.id)? {
            if let Some(s) = ch.summary {
                let line = format!("- [{}] {}\n", format_time(ch.start), s);
                if estimate_tokens(&block) + estimate_tokens(&parts) + estimate_tokens(&line) > PER_TRANSCRIPT_TOKENS {
                    break;
                }
                parts.push_str(&line);
            }
        }
        if !parts.is_empty() {
            block.push_str("\nPart by part:\n");
            block.push_str(&parts);
        }
        m.text.push_str(&block);
        m.text.push('\n');
        m.allowed.insert(t.id.clone(), vec![(0.0, t.duration + 1.0)]);
        m.titles.insert(t.id.clone(), t.title.clone());
        m.sources.push(format!("- {} (id {})", t.title, t.id));
    }
    Ok(m)
}

/// Generate a document from `template_id` for `scope`. Stored as digest `doc:<id>` for a single
/// transcript so it can be reopened without paying again (until the transcript changes).
pub async fn generate(ctx: &AiCtx<'_>, scope: &Scope, template_id: &str) -> Result<String> {
    let template = templates(ctx.settings).into_iter().find(|t| t.id == template_id).ok_or_else(|| anyhow!("Unknown template {template_id:?}"))?;
    let cfg = ctx.chat_provider()?;
    let key = format!("doc:{template_id}");
    let m = material(ctx, scope)?;
    if m.allowed.is_empty() {
        bail!("None of these transcripts has a summary yet. Summarise them first (Assistant → Summary).");
    }
    ctx.progress(&key, format!("Writing: {}", template.name), 0.2);
    let system = [SystemBlock { text: format!("{}{RULES}", template.prompt), cache: false }];
    let msg = [ChatMessage { role: Role::User, content: format!("Meeting material:\n\n{}", m.text) }];
    let req = ChatRequest { system: &system, messages: &msg, max_tokens: 8192, json: false, cached_content: None, temperature: Some(0.2) };
    let on_retry = |msg: String| ctx.progress(&key, msg, 0.3);
    let call = ChatCall { cancel: ctx.cancel, on_retry: &on_retry, on_delta: None };
    let r = match complete(ctx.http, &cfg, &req, &call).await {
        Ok(r) => r,
        Err(e) => {
            ctx.done(&key, "", Some(format!("{e:#}")));
            return Err(e);
        }
    };
    let title_refs: HashMap<&str, &str> = m.titles.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (body, _) = validate_citations(&r.text, &m.allowed, &title_refs);
    let mut out = format!("{body}\n\n---\nSources:\n{}\n", m.sources.join("\n"));
    if !m.missing.is_empty() {
        out.push_str(&format!("\nNot included (no summary yet): {}\n", m.missing.join(", ")));
    }
    out.push_str(&format!("\n_{} · generated {} by {} from the Transcriber library._\n", template.name, chrono::Local::now().format("%Y-%m-%d"), cfg.model));
    {
        let c = ctx.db();
        if let Some(id) = scope.transcript_id() {
            if let Some(h) = store::transcript_hash(&c, id)? {
                store::put_digest(&c, id, &format!("doc:{template_id}"), &h, &cfg.model, &out)?;
            }
        }
        usage::record(&c, &usage::Record { kind: "ai", cfg: &cfg, usage: r.usage, requests: 1, status: "done", transcript_id: scope.transcript_id(), title: &format!("Document: {}", template.name) });
    }
    ctx.done(&key, format!("{} ready", template.name), None);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_templates_override_builtins() {
        let mut s = Settings::default();
        s.ai_templates.push(crate::settings::AiTemplate { id: "mom".into(), name: "Our minutes".into(), description: String::new(), prompt: "x".into() });
        s.ai_templates.push(crate::settings::AiTemplate { id: "retro".into(), name: "Retro".into(), description: String::new(), prompt: "y".into() });
        let all = templates(&s);
        assert_eq!(all.iter().filter(|t| t.id == "mom").count(), 1);
        assert_eq!(all.iter().find(|t| t.id == "mom").unwrap().name, "Our minutes");
        assert!(all.iter().any(|t| t.id == "retro" && !t.builtin));
        assert!(builtin_templates().iter().all(|t| !t.prompt.is_empty()));
    }
}
