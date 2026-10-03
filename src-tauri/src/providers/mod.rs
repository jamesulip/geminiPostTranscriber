pub mod gemini;
pub mod gemini_batch;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcription {
    pub segments: Vec<Segment>,
    pub usage: TokenUsage,
    /// Language detected by the provider, if reported (e.g. "english" or "en-US").
    pub language: Option<String>,
    /// Raw provider response, saved for diagnosis (not cached).
    #[serde(skip)]
    pub raw: Option<serde_json::Value>,
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
    // Interactions API: `*_tokens_by_modality: [{modality, tokens}]`.
    let by_modality = |key: &str| -> u64 {
        u.get(key)
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|m| m.get("tokens").or_else(|| m.get("token_count"))?.as_u64()).sum())
            .unwrap_or(0)
    };
    let mut input = get(&["input_tokens", "total_input_tokens", "promptTokenCount", "prompt_token_count"]);
    if input == 0 {
        input = by_modality("input_tokens_by_modality");
    }
    let mut output = get(&["output_tokens", "total_output_tokens", "candidatesTokenCount", "candidates_token_count"]);
    if output == 0 {
        output = by_modality("output_tokens_by_modality");
    }
    if output == 0 {
        // Last resort: total minus input (and thinking/cached, which aren't billed as output here).
        let total = get(&["total_tokens", "totalTokenCount", "total_token_count"]);
        let other = get(&["total_thought_tokens", "thoughtsTokenCount"]) + get(&["total_cached_tokens", "cachedContentTokenCount"]);
        output = total.saturating_sub(input + other);
    }
    // Thinking ("thought") tokens are billed at the output rate but reported separately.
    let thoughts = get(&["total_thought_tokens", "thoughtsTokenCount", "thoughts_token_count"]);
    TokenUsage { input_tokens: input, output_tokens: output + thoughts }
}

pub struct Request<'a> {
    pub path: &'a Path,
    pub duration: f64,
    pub language: Option<&'a str>,
    /// Speakers seen in earlier chunks, for prompt-based models.
    pub known_speakers: &'a [String],
    pub previous_text: &'a str,
    /// Called with a status message when waiting before a retry (e.g. rate limits).
    pub on_retry: &'a (dyn Fn(String) + Send + Sync),
    /// Set by the Cancel button; checked during requests and retry waits.
    pub cancel: &'a std::sync::atomic::AtomicBool,
}

/// Resolves once `flag` is set (polled; cheap).
async fn cancelled(flag: &std::sync::atomic::AtomicBool) {
    while !flag.load(std::sync::atomic::Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Retries for server/network errors.
const MAX_ATTEMPTS: u32 = 6;
/// Rate limits on long recordings are expected; keep waiting as long as the provider asks,
/// up to this much total waiting per chunk.
const RATE_LIMIT_BUDGET_SECS: u64 = 45 * 60;

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
    let mut waited = 0u64;
    loop {
        if req.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let call = async {
            match cfg.kind {
                ProviderKind::Openai => openai_compat::transcribe(client, cfg, req).await,
                ProviderKind::Gemini => gemini::transcribe(client, cfg, req).await,
            }
        };
        // Cancel stops an in-flight request too (it can take minutes for long chunks).
        let res = tokio::select! {
            r = call => r,
            _ = cancelled(req.cancel) => bail!("Cancelled"),
        };
        let http = res.as_ref().err().and_then(|e| e.downcast_ref::<HttpError>());
        let rate_limited = http.is_some_and(|h| h.status == 429 && !h.daily);
        let retry = match &res {
            Err(_) if rate_limited => waited < RATE_LIMIT_BUDGET_SECS,
            Err(e) => attempt + 1 < MAX_ATTEMPTS && is_retryable(e),
            Ok(_) => false,
        };
        match res {
            Err(e) if retry => {
                attempt += 1;
                // Honour the provider's delay (Retry-After / RetryInfo), else back off exponentially.
                let hinted = e.downcast_ref::<HttpError>().and_then(|h| h.retry_after);
                let wait = hinted.unwrap_or(2u64.pow(attempt.min(8))).clamp(1, 300);
                waited += wait;
                // Outermost context names the step, e.g. "uploading audio" / "transcribing".
                let step = if e.chain().count() > 1 { format!(" while {e}") } else { String::new() };
                let why = match e.downcast_ref::<HttpError>() {
                    Some(h) if h.status == 429 => match &h.quota {
                        Some(q) => format!("{} quota reached{step}: {q}", cfg.name),
                        None => format!("Rate limited by {}{step}", cfg.name),
                    },
                    Some(h) => format!("{} returned HTTP {}", cfg.name, h.status),
                    None => format!("Network error talking to {}", cfg.name),
                };
                let tries = if rate_limited { format!("waited {}m so far", waited / 60) } else { format!("attempt {}/{MAX_ATTEMPTS}", attempt + 1) };
                (req.on_retry)(format!("{why} — retrying in {wait}s ({tries})"));
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                    _ = cancelled(req.cancel) => bail!("Cancelled"),
                }
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
    /// Seconds to wait, from the `Retry-After` header or Google's `RetryInfo.retryDelay`.
    pub retry_after: Option<u64>,
    /// Which quota was exceeded (Google `QuotaFailure`), e.g. "input tokens per minute (limit 250000)".
    pub quota: Option<String>,
    /// The quota resets daily: waiting a few minutes won't help.
    pub daily: bool,
}

/// Pull retry delay and quota details out of a Google API error body.
fn parse_google_error(body: &str) -> (Option<u64>, Option<String>, bool) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return (None, None, false) };
    let details = v["error"]["details"].as_array().cloned().unwrap_or_default();
    let mut retry = None;
    let mut quota = None;
    let mut daily = false;
    for d in &details {
        if let Some(delay) = d["retryDelay"].as_str() {
            retry = delay.trim_end_matches('s').parse::<f64>().ok().map(|s| s.ceil() as u64);
        }
        if let Some(violation) = d["violations"].as_array().and_then(|a| a.first()) {
            let id = violation["quotaId"].as_str().unwrap_or_default();
            daily = id.contains("PerDay");
            quota = Some(describe_quota(id, violation["quotaValue"].as_str()));
        }
    }
    (retry, quota, daily)
}

/// "GenerateContentInputTokensPerModelPerMinute-FreeTier" → "input tokens per minute (free tier, limit 250000)"
fn describe_quota(id: &str, value: Option<&str>) -> String {
    let what = if id.contains("InputToken") {
        "input tokens"
    } else if id.contains("Token") {
        "tokens"
    } else if id.contains("Request") {
        "requests"
    } else {
        "usage"
    };
    let per = if id.contains("PerDay") { "per day" } else if id.contains("PerMinute") { "per minute" } else { "" };
    let tier = if id.contains("FreeTier") { "free tier, " } else { "" };
    let limit = value.map(|v| format!("{tier}limit {v}")).unwrap_or_else(|| tier.trim_end_matches(", ").to_string());
    let mut s = format!("{what} {per}").trim().to_string();
    if !limit.is_empty() {
        s.push_str(&format!(" ({limit})"));
    }
    s
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status == 429 {
            let quota = self.quota.as_deref().map(|q| format!(" — quota: {q}")).unwrap_or_default();
            let hint = if self.daily {
                ". The daily quota resets tomorrow; finished chunks are saved, so retrying later resumes where it stopped"
            } else {
                ""
            };
            return write!(f, "Rate limited (HTTP 429){quota}{hint}");
        }
        write!(f, "HTTP {}: {}", self.status, self.body.chars().take(500).collect::<String>())
    }
}
impl std::error::Error for HttpError {}

fn is_retryable(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<HttpError>() {
        Some(h) => (h.status == 429 && !h.daily) || h.status >= 500,
        None => e.downcast_ref::<reqwest::Error>().is_some_and(|r| r.is_timeout() || r.is_connect()),
    }
}

pub async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|v| v.ceil() as u64);
    let body = resp.text().await.unwrap_or_default();
    let (google_retry, quota, daily) = parse_google_error(&body);
    Err(HttpError { status: status.as_u16(), body, retry_after: retry_after.or(google_retry), quota, daily }.into())
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

    /// Cancel must stop a request that never answers (and not wait for timeouts or retries).
    #[tokio::test]
    async fn cancel_interrupts_hanging_request() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        // A server that accepts connections and never responds.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });
        let audio = std::env::temp_dir().join(format!("cancel-test-{}.ogg", uuid::Uuid::new_v4()));
        std::fs::write(&audio, b"fake").unwrap();
        let cfg = ProviderConfig {
            id: "t".into(), name: "Test".into(), kind: ProviderKind::Openai, base_url: format!("http://{addr}"),
            api_key: "k".into(), model: "m".into(), price_per_minute: None, price_input_per_mtok: None, price_output_per_mtok: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            c2.store(true, Ordering::Relaxed);
        });
        let req = Request {
            path: &audio, duration: 10.0, language: None, known_speakers: &[], previous_text: "",
            on_retry: &|_| {}, cancel: &cancel,
        };
        let start = std::time::Instant::now();
        let err = transcribe(&reqwest::Client::new(), &cfg, &req).await.unwrap_err();
        assert_eq!(err.to_string(), "Cancelled");
        assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
        let _ = std::fs::remove_file(audio);
    }

    #[test]
    fn parses_google_rate_limit_errors() {
        let body = r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","details":[
            {"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{"quotaMetric":"generativelanguage.googleapis.com/generate_content_free_tier_input_token_count","quotaId":"GenerateContentInputTokensPerModelPerMinute-FreeTier","quotaValue":"250000"}]},
            {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"55.6s"}]}}"#;
        let (retry, quota, daily) = parse_google_error(body);
        assert_eq!(retry, Some(56));
        assert_eq!(quota.as_deref(), Some("input tokens per minute (free tier, limit 250000)"));
        assert!(!daily);
        let day = body.replace("GenerateContentInputTokensPerModelPerMinute", "GenerateRequestsPerDayPerProjectPerModel");
        let (_, quota, daily) = parse_google_error(&day);
        assert!(daily && quota.unwrap().starts_with("requests per day"));
        let e: anyhow::Error = HttpError { status: 429, body: day, retry_after: None, quota: Some("requests per day".into()), daily: true }.into();
        assert!(!is_retryable(&e));
        assert!(e.to_string().contains("resumes where it stopped"));
        assert_eq!(parse_google_error("not json"), (None, None, false));
    }

    #[test]
    fn extracts_interaction_usage_fallbacks() {
        let by_mod = extract_usage(&json!({"usage": {"input_tokens_by_modality": [{"modality": "audio", "tokens": 1500}],
            "output_tokens_by_modality": [{"modality": "text", "tokens": 175}]}}));
        assert_eq!(by_mod, TokenUsage { input_tokens: 1500, output_tokens: 175 });
        let total = extract_usage(&json!({"usage": {"total_input_tokens": 1500, "total_tokens": 1700, "total_thought_tokens": 25}}));
        assert_eq!(total.output_tokens, 200, "175 text + 25 thinking");
        // generateContent: candidates + thoughts both count as output.
        let g = extract_usage(&json!({"usageMetadata": {"promptTokenCount": 14797, "candidatesTokenCount": 9804, "thoughtsTokenCount": 4784}}));
        assert_eq!((g.input_tokens, g.output_tokens), (14797, 9804 + 4784));
        // Blocked response: input + thinking still billed, no text.
        let b = extract_usage(&json!({"promptFeedback": {"blockReason": "OTHER"}, "usageMetadata": {"promptTokenCount": 14405, "thoughtsTokenCount": 4715}}));
        assert_eq!((b.input_tokens, b.output_tokens), (14405, 4715));
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
