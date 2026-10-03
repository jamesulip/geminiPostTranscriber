//! One `complete()` for every chat provider: Gemini `generateContent`, OpenAI-compatible
//! `/chat/completions` (OpenAI, Groq, local servers) and the Anthropic Messages API.
//! Streaming is on whenever `on_delta` is given.

use super::sse::read_sse;
use crate::providers::retry::{with_retry, RetryCtx, RetryPolicy};
use crate::providers::{check, extract_usage, TokenUsage};
use crate::settings::{ProviderConfig, ProviderKind};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// A piece of the system prompt. `cache: true` marks a large, stable block (a whole transcript)
/// for the provider's prompt cache where that is explicit (Anthropic `cache_control`).
#[derive(Debug, Clone)]
pub struct SystemBlock {
    pub text: String,
    pub cache: bool,
}

pub struct ChatRequest<'a> {
    pub system: &'a [SystemBlock],
    pub messages: &'a [ChatMessage],
    pub max_tokens: u32,
    /// Ask for a JSON object (native where the API supports it; the prompt must say so too).
    pub json: bool,
    /// Gemini `cachedContents` name holding the system prompt + context; `system` is then omitted.
    pub cached_content: Option<&'a str>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub text: String,
    pub usage: TokenUsage,
}

pub struct ChatCall<'a> {
    pub cancel: &'a AtomicBool,
    pub on_retry: &'a (dyn Fn(String) + Send + Sync),
    /// Receives text as it arrives; enables streaming.
    pub on_delta: Option<&'a (dyn Fn(&str) + Send + Sync)>,
}

const TIMEOUT: Duration = Duration::from_secs(600);

pub async fn complete(http: &reqwest::Client, cfg: &ProviderConfig, req: &ChatRequest<'_>, call: &ChatCall<'_>) -> Result<ChatResponse> {
    if cfg.api_key.trim().is_empty() {
        bail!("No API key set for {}. Add one in Settings.", cfg.name);
    }
    let policy = RetryPolicy::interactive();
    let ctx = RetryCtx { cancel: call.cancel, on_retry: call.on_retry, provider_name: &cfg.name };
    with_retry(&policy, &ctx, || async move {
        match cfg.kind {
            ProviderKind::Gemini => gemini(http, cfg, req, call).await,
            ProviderKind::Openai => openai(http, cfg, req, call).await,
            ProviderKind::Anthropic => anthropic(http, cfg, req, call).await,
        }
    })
    .await
    .with_context(|| format!("asking {} ({})", cfg.name, cfg.model))
}

fn system_text(req: &ChatRequest<'_>) -> String {
    req.system.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n\n")
}

// Gemini ----------------------------------------------------------------------------------

fn gemini_text(v: &Value) -> String {
    v["candidates"][0]["content"]["parts"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| !p["thought"].as_bool().unwrap_or(false))
                .filter_map(|p| p["text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn gemini_check(v: &Value, text_empty: bool) -> Result<()> {
    if let Some(reason) = v["promptFeedback"]["blockReason"].as_str() {
        bail!("Google blocked this request ({reason})");
    }
    if text_empty {
        let reason = v["candidates"][0]["finishReason"].as_str().unwrap_or("unknown");
        if reason == "MAX_TOKENS" {
            bail!("the answer was cut off before any text (output limit)");
        }
        bail!("Gemini returned no text (finish reason: {reason})");
    }
    Ok(())
}

async fn gemini(http: &reqwest::Client, cfg: &ProviderConfig, req: &ChatRequest<'_>, call: &ChatCall<'_>) -> Result<ChatResponse> {
    let contents: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({ "role": if m.role == Role::User { "user" } else { "model" }, "parts": [{ "text": m.content }] }))
        .collect();
    let mut gen = json!({ "maxOutputTokens": req.max_tokens });
    if req.json {
        gen["responseMimeType"] = json!("application/json");
    }
    if let Some(t) = req.temperature {
        gen["temperature"] = json!(t);
    }
    let mut body = json!({ "contents": contents, "generationConfig": gen });
    match req.cached_content {
        Some(name) => body["cachedContent"] = json!(name),
        None if !req.system.is_empty() => body["systemInstruction"] = json!({ "parts": [{ "text": system_text(req) }] }),
        None => {}
    }
    let stream = call.on_delta.is_some();
    let method = if stream { "streamGenerateContent?alt=sse" } else { "generateContent" };
    let url = format!("{}/v1beta/models/{}:{method}", cfg.base_url.trim_end_matches('/'), cfg.model);
    let resp = http.post(url).header("x-goog-api-key", &cfg.api_key).timeout(TIMEOUT).json(&body).send().await?;
    let resp = check(resp).await?;
    if !stream {
        let v: Value = resp.json().await?;
        let text = gemini_text(&v);
        gemini_check(&v, text.trim().is_empty())?;
        return Ok(ChatResponse { text, usage: extract_usage(&v) });
    }
    let on_delta = call.on_delta.unwrap();
    let mut out = ChatResponse::default();
    let mut last = Value::Null;
    read_sse(resp, |d| {
        let v: Value = serde_json::from_str(d).context("bad event from Gemini")?;
        let t = gemini_text(&v);
        if !t.is_empty() {
            on_delta(&t);
            out.text.push_str(&t);
        }
        let u = extract_usage(&v);
        if u.input_tokens > 0 || u.output_tokens > 0 {
            out.usage = u;
        }
        last = v;
        Ok(())
    })
    .await?;
    gemini_check(&last, out.text.trim().is_empty())?;
    Ok(out)
}

// OpenAI-compatible -----------------------------------------------------------------------

async fn openai(http: &reqwest::Client, cfg: &ProviderConfig, req: &ChatRequest<'_>, call: &ChatCall<'_>) -> Result<ChatResponse> {
    let mut messages: Vec<Value> = Vec::new();
    if !req.system.is_empty() {
        messages.push(json!({ "role": "system", "content": system_text(req) }));
    }
    messages.extend(
        req.messages
            .iter()
            .map(|m| json!({ "role": if m.role == Role::User { "user" } else { "assistant" }, "content": m.content })),
    );
    let stream = call.on_delta.is_some();
    let mut body = json!({ "model": cfg.model, "messages": messages, "max_tokens": req.max_tokens, "stream": stream });
    if stream {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if req.json {
        body["response_format"] = json!({ "type": "json_object" });
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let resp = http.post(url).bearer_auth(&cfg.api_key).timeout(TIMEOUT).json(&body).send().await?;
    let resp = check(resp).await?;
    if !stream {
        let v: Value = resp.json().await?;
        let text = v["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string();
        if text.trim().is_empty() {
            bail!("{} returned no text", cfg.name);
        }
        return Ok(ChatResponse { text, usage: extract_usage(&v) });
    }
    let on_delta = call.on_delta.unwrap();
    let mut out = ChatResponse::default();
    read_sse(resp, |d| {
        if d.trim() == "[DONE]" {
            return Ok(());
        }
        let v: Value = serde_json::from_str(d).context("bad event from provider")?;
        if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
            if !t.is_empty() {
                on_delta(t);
                out.text.push_str(t);
            }
        }
        if v.get("usage").is_some_and(|u| !u.is_null()) {
            out.usage = extract_usage(&v);
        }
        Ok(())
    })
    .await?;
    if out.text.trim().is_empty() {
        bail!("{} returned no text", cfg.name);
    }
    Ok(out)
}

// Anthropic -------------------------------------------------------------------------------

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

fn anthropic_url(cfg: &ProviderConfig) -> String {
    let base = cfg.base_url.trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    format!("{base}/v1/messages")
}

async fn anthropic(http: &reqwest::Client, cfg: &ProviderConfig, req: &ChatRequest<'_>, call: &ChatCall<'_>) -> Result<ChatResponse> {
    let system: Vec<Value> = req
        .system
        .iter()
        .map(|b| {
            let mut block = json!({ "type": "text", "text": b.text });
            if b.cache {
                block["cache_control"] = json!({ "type": "ephemeral" });
            }
            block
        })
        .collect();
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({ "role": if m.role == Role::User { "user" } else { "assistant" }, "content": m.content }))
        .collect();
    let stream = call.on_delta.is_some();
    let mut body = json!({ "model": cfg.model, "max_tokens": req.max_tokens, "messages": messages, "stream": stream });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    let resp = http
        .post(anthropic_url(cfg))
        .header("x-api-key", &cfg.api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .timeout(TIMEOUT)
        .json(&body)
        .send()
        .await?;
    let resp = check(resp).await?;
    if !stream {
        let v: Value = resp.json().await?;
        if v["stop_reason"].as_str() == Some("refusal") {
            bail!("Claude declined this request ({})", v["stop_details"]["category"].as_str().unwrap_or("safety"));
        }
        let text: String =
            v["content"].as_array().map(|c| c.iter().filter_map(|b| b["text"].as_str()).collect()).unwrap_or_default();
        if text.trim().is_empty() {
            bail!("Claude returned no text (stop reason: {})", v["stop_reason"].as_str().unwrap_or("unknown"));
        }
        return Ok(ChatResponse { text, usage: extract_usage(&v) });
    }
    let on_delta = call.on_delta.unwrap();
    let mut out = ChatResponse::default();
    let mut stop = None;
    read_sse(resp, |d| {
        let v: Value = serde_json::from_str(d).context("bad event from Anthropic")?;
        match v["type"].as_str().unwrap_or("") {
            "message_start" => out.usage = extract_usage(&v["message"]),
            "content_block_delta" => {
                if let Some(t) = v["delta"]["text"].as_str() {
                    on_delta(t);
                    out.text.push_str(t);
                }
            }
            "message_delta" => {
                let u = extract_usage(&v);
                if u.output_tokens > 0 {
                    out.usage.output_tokens = u.output_tokens;
                }
                if let Some(s) = v["delta"]["stop_reason"].as_str() {
                    stop = Some(s.to_string());
                }
            }
            "error" => bail!("Anthropic error: {}", v["error"]["message"].as_str().unwrap_or("unknown")),
            _ => {}
        }
        Ok(())
    })
    .await?;
    if stop.as_deref() == Some("refusal") {
        bail!("Claude declined this request");
    }
    if out.text.trim().is_empty() {
        bail!("Claude returned no text");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_url_tolerates_v1_suffix() {
        let mut p = crate::settings::anthropic_provider();
        assert_eq!(anthropic_url(&p), "https://api.anthropic.com/v1/messages");
        p.base_url = "http://127.0.0.1:8767/v1/".into();
        assert_eq!(anthropic_url(&p), "http://127.0.0.1:8767/v1/messages");
    }

    #[test]
    fn reads_gemini_text_and_skips_thoughts() {
        let v = json!({ "candidates": [{ "content": { "parts": [{ "text": "hmm", "thought": true }, { "text": "Hello" }] } }] });
        assert_eq!(gemini_text(&v), "Hello");
        let blocked = json!({ "promptFeedback": { "blockReason": "OTHER" } });
        assert!(gemini_check(&blocked, true).unwrap_err().to_string().contains("blocked"));
    }
}
