pub mod gemini;
pub mod openai_compat;

use crate::settings::{ProviderConfig, ProviderKind};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// A transcript segment. Times are relative to the audio that was sent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub speaker: Option<String>,
}

/// Token counts if the provider reports them (0 otherwise).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl std::ops::AddAssign for TokenUsage {
    fn add_assign(&mut self, o: Self) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
    }
}

#[derive(Debug, Clone)]
pub struct Transcription {
    pub segments: Vec<Segment>,
    pub usage: TokenUsage,
    /// Language detected by the provider, if reported (e.g. "english" or "en-US").
    pub language: Option<String>,
}

/// Find a detected-language field anywhere in a response.
pub fn extract_language(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(m) => {
            for key in ["language", "language_code", "detected_language", "languageCode"] {
                if let Some(l) = m.get(key).and_then(|x| x.as_str()).filter(|l| !l.trim().is_empty()) {
                    return Some(l.trim().to_string());
                }
            }
            m.values().find_map(extract_language)
        }
        serde_json::Value::Array(a) => a.iter().find_map(extract_language),
        _ => None,
    }
}

/// Find a usage block in a provider response. Handles OpenAI (`usage.input_tokens`),
/// Gemini generateContent (`usageMetadata.promptTokenCount`) and Interactions-style
/// (`usage.total_input_tokens`) shapes.
pub fn extract_usage(v: &serde_json::Value) -> TokenUsage {
    fn find<'a>(v: &'a serde_json::Value) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
        let m = v.as_object()?;
        for key in ["usage", "usageMetadata", "usage_metadata"] {
            if let Some(u) = m.get(key).and_then(|u| u.as_object()) {
                return Some(u);
            }
        }
        m.values().find_map(|c| match c {
            serde_json::Value::Object(_) => find(c),
            serde_json::Value::Array(a) => a.iter().find_map(find),
            _ => None,
        })
    }
    let Some(u) = find(v) else { return TokenUsage::default() };
    let get = |keys: &[&str]| keys.iter().find_map(|k| u.get(*k).and_then(|x| x.as_u64())).unwrap_or(0);
    TokenUsage {
        input_tokens: get(&["input_tokens", "total_input_tokens", "promptTokenCount", "prompt_token_count"]),
        output_tokens: get(&["output_tokens", "total_output_tokens", "candidatesTokenCount", "candidates_token_count"]),
    }
}

pub struct Request<'a> {
    pub path: &'a Path,
    pub duration: f64,
    pub language: Option<&'a str>,
    /// Speakers seen in earlier chunks, for prompt-based models.
    pub known_speakers: &'a [String],
    pub previous_text: &'a str,
}

/// Whether chunks can be sent concurrently. Gemini runs sequentially so speaker
/// context from the previous chunk can be passed along.
pub fn parallel(cfg: &ProviderConfig) -> bool {
    cfg.kind == ProviderKind::Openai
}

pub async fn transcribe(client: &reqwest::Client, cfg: &ProviderConfig, req: &Request<'_>) -> Result<Transcription> {
    if cfg.api_key.trim().is_empty() {
        bail!("No API key set for {}. Add one in Settings.", cfg.name);
    }
    let mut attempt = 0;
    loop {
        let res = match cfg.kind {
            ProviderKind::Openai => openai_compat::transcribe(client, cfg, req).await,
            ProviderKind::Gemini => gemini::transcribe(client, cfg, req).await,
        };
        match res {
            Err(e) if attempt < 3 && is_retryable(&e) => {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
            }
            other => {
                return other.map(|t| Transcription { segments: clamp(t.segments, req.duration), ..t })
            }
        }
    }
}

#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub body: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.body.chars().take(500).collect::<String>())
    }
}
impl std::error::Error for HttpError {}

fn is_retryable(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<HttpError>() {
        Some(h) => h.status == 429 || h.status >= 500,
        None => e.downcast_ref::<reqwest::Error>().is_some_and(|r| r.is_timeout() || r.is_connect()),
    }
}

pub async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(HttpError { status: status.as_u16(), body }.into())
}

/// Models sometimes drift past the end of the audio; keep segments within bounds.
fn clamp(segs: Vec<Segment>, duration: f64) -> Vec<Segment> {
    segs.into_iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|mut s| {
            s.start = s.start.clamp(0.0, duration);
            s.end = s.end.clamp(s.start, duration);
            s.text = s.text.trim().to_string();
            s
        })
        .collect()
}

/// Accepts "1.5s", "75.2", "01:15.2", "1:02:03.5".
pub fn parse_time(v: &serde_json::Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    let s = v.as_str()?.trim().trim_end_matches('s');
    let mut total = 0.0;
    for part in s.split(':') {
        total = total * 60.0 + part.trim().parse::<f64>().ok()?;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_time_formats() {
        assert_eq!(parse_time(&json!("0.450s")), Some(0.45));
        assert_eq!(parse_time(&json!(12.5)), Some(12.5));
        assert_eq!(parse_time(&json!("01:15.5")), Some(75.5));
        assert_eq!(parse_time(&json!("1:02:03")), Some(3723.0));
        assert_eq!(parse_time(&json!("abc")), None);
    }

    #[test]
    fn extracts_usage_shapes() {
        let o = extract_usage(&json!({"text": "x", "usage": {"type": "tokens", "input_tokens": 10, "output_tokens": 3}}));
        assert_eq!(o, TokenUsage { input_tokens: 10, output_tokens: 3 });
        let g = extract_usage(&json!({"candidates": [], "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 2}}));
        assert_eq!(g, TokenUsage { input_tokens: 7, output_tokens: 2 });
        let i = extract_usage(&json!({"steps": [{"usage": {"total_input_tokens": 5, "total_output_tokens": 1}}]}));
        assert_eq!(i, TokenUsage { input_tokens: 5, output_tokens: 1 });
        assert_eq!(extract_usage(&json!({"text": "x", "duration": 3.0})), TokenUsage::default());
        assert_eq!(extract_language(&json!({"text": "x", "language": "english"})).as_deref(), Some("english"));
        assert_eq!(extract_language(&json!({"steps": [{"language_code": "fil-PH"}]})).as_deref(), Some("fil-PH"));
        assert_eq!(extract_language(&json!({"text": "x"})), None);
    }
}
