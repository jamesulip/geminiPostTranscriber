//! AI assistant harness: one place every AI feature (summaries, questions, semantic search,
//! the MCP server) goes through. Nothing here imports Tauri; the app passes an `on_event`
//! callback that forwards events to the window.
//!
//! Three cache tiers keep the model from re-reading transcripts:
//! 1. derived artifacts (`ai_chunks.summary`, `ai_digests`) keyed by content hash;
//! 2. retrieval (`ai_chunks.embedding` + FTS5) so a question sends a few excerpts;
//! 3. the provider's own prompt cache for "read the whole meeting" sessions.

pub mod ask;
pub mod brief;
pub mod cache;
pub mod chat;
pub mod chunker;
pub mod digest;
pub mod embed;
pub mod prompts;
pub mod retrieval;
pub mod sse;
pub mod store;
pub mod translate;
pub mod usage;
#[cfg(test)]
mod tests;

use crate::settings::{ProviderConfig, ProviderKind, Settings};
use anyhow::{anyhow, bail, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard};

/// Events for the UI. `Token` streams an answer; `Progress` reports background work
/// (keys: `digest:<transcript id>`, `embed:library`, `ask:<thread id>`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AiEvent {
    Token { thread_id: String, delta: String },
    Progress { key: String, message: String, progress: f64, done: bool, error: Option<String> },
}

pub struct AiCtx<'a> {
    pub http: &'a reqwest::Client,
    pub db: &'a Mutex<Connection>,
    pub settings: &'a Settings,
    /// Set to stop the current operation (request in flight included).
    pub cancel: &'a AtomicBool,
    pub on_event: &'a (dyn Fn(AiEvent) + Send + Sync),
}

impl AiCtx<'_> {
    /// Lock the library. Never hold the guard across an `.await`.
    pub fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The assistant's chat provider, with its chat model in `model`.
    pub fn chat_provider(&self) -> Result<ProviderConfig> {
        let s = self.settings;
        let mut p = s
            .providers
            .iter()
            .find(|p| p.id == s.ai_provider)
            .cloned()
            .ok_or_else(|| anyhow!("No AI provider selected. Pick one under Settings → AI assistant."))?;
        if !s.ai_model.trim().is_empty() {
            p.model = s.ai_model.trim().to_string();
        } else {
            p.model = crate::settings::default_chat_model(&p.kind).to_string();
        }
        if p.api_key.trim().is_empty() {
            bail!("No API key set for {}. Add one in Settings.", p.name);
        }
        Ok(p)
    }

    /// The embeddings provider (semantic search), with the embedding model in `model`.
    pub fn embed_provider(&self) -> Result<ProviderConfig> {
        let s = self.settings;
        let mut p = s
            .providers
            .iter()
            .find(|p| p.id == s.ai_embedding_provider)
            .cloned()
            .ok_or_else(|| anyhow!("No embeddings provider selected. Pick one under Settings → AI assistant."))?;
        if p.kind == ProviderKind::Anthropic {
            bail!("Anthropic has no embeddings API; pick Gemini or an OpenAI-compatible provider for embeddings.");
        }
        if s.ai_embedding_model.trim().is_empty() {
            bail!("No embedding model set under Settings → AI assistant.");
        }
        p.model = s.ai_embedding_model.trim().to_string();
        if p.api_key.trim().is_empty() {
            bail!("No API key set for {}. Add one in Settings.", p.name);
        }
        Ok(p)
    }

    pub fn progress(&self, key: &str, message: impl Into<String>, progress: f64) {
        (self.on_event)(AiEvent::Progress { key: key.to_string(), message: message.into(), progress, done: false, error: None });
    }

    pub fn done(&self, key: &str, message: impl Into<String>, error: Option<String>) {
        (self.on_event)(AiEvent::Progress { key: key.to_string(), message: message.into(), progress: 1.0, done: true, error });
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// What a question or search is about.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Scope {
    Transcript { id: String },
    Folder { id: i64 },
    Tag { id: i64 },
    Library,
}

impl Scope {
    pub fn transcript_id(&self) -> Option<&str> {
        match self {
            Scope::Transcript { id } => Some(id),
            _ => None,
        }
    }
}

/// Rough token count. Filipino/Taglish tokenises worse than English; ~3 characters per token
/// keeps budgets conservative.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count() / 3 + 1
}

/// "1:02:33" / "12:05"
pub fn format_time(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// Strip ```json fences a model may wrap JSON in, then parse.
pub fn parse_json_lenient(text: &str) -> Result<serde_json::Value> {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    if let Ok(v) = serde_json::from_str(t) {
        return Ok(v);
    }
    // Some models add prose around the object: take the outermost braces.
    let (a, b) = (t.find('{'), t.rfind('}'));
    if let (Some(a), Some(b)) = (a, b) {
        if a < b {
            return Ok(serde_json::from_str(&t[a..=b])?);
        }
    }
    bail!("the model did not return valid JSON")
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn formats_times_and_parses_fenced_json() {
        assert_eq!(format_time(3753.4), "1:02:33");
        assert_eq!(format_time(65.0), "1:05");
        let v = parse_json_lenient("Sure!\n```json\n{\"a\": 1}\n```").unwrap();
        assert_eq!(v["a"], 1);
        assert!(parse_json_lenient("nothing here").is_err());
        let s: Scope = serde_json::from_str(r#"{"kind":"folder","id":3}"#).unwrap();
        assert_eq!(s, Scope::Folder { id: 3 });
    }
}
