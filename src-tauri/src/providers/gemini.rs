//! Gemini. `*-transcribe` models use the Interactions API (native diarization and word
//! timestamps); other models fall back to `generateContent` with a JSON schema.

use super::{check, extract_language, extract_usage, parse_time, Request, Segment, TokenUsage, Transcription};
use crate::settings::{ProviderConfig, ProviderKind};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::time::Duration;

const MIME: &str = "audio/ogg";
/// No real word lasts longer than this.
const MAX_WORD_SECS: f64 = 10.0;

pub async fn transcribe(client: &reqwest::Client, cfg: &ProviderConfig, req: &Request<'_>) -> Result<Transcription> {
    if cfg.model.contains("transcribe") {
        transcribe_model(client, cfg, req).await
    } else {
        generate_content(client, cfg, req).await
    }
}

pub fn base(cfg: &ProviderConfig) -> &str {
    cfg.base_url.trim_end_matches('/')
}

// ---------------------------------------------------------------------------
// Interactions API (gemini-3.5-transcribe)
// ---------------------------------------------------------------------------

async fn transcribe_model(client: &reqwest::Client, cfg: &ProviderConfig, req: &Request<'_>) -> Result<Transcription> {
    let file = upload(client, cfg, req).await.context("uploading audio")?;
    let result = interact(client, cfg, req, &file).await.context("transcribing");
    // Best-effort cleanup; uploaded files otherwise expire after 48h.
    let _ = client
        .delete(format!("{}/v1beta/{}", base(cfg), file.name))
        .header("x-goog-api-key", &cfg.api_key)
        .send()
        .await;
    result
}

pub struct UploadedFile {
    pub name: String,
    pub uri: String,
}

async fn upload(client: &reqwest::Client, cfg: &ProviderConfig, req: &Request<'_>) -> Result<UploadedFile> {
    let bytes = tokio::fs::read(req.path).await?;
    upload_bytes(client, cfg, bytes, MIME, "transcriber-chunk").await
}

/// Upload bytes with the Files API (resumable protocol) and wait until the file is usable.
pub async fn upload_bytes(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    bytes: Vec<u8>,
    mime: &str,
    display_name: &str,
) -> Result<UploadedFile> {
    let start = client
        .post(format!("{}/upload/v1beta/files", base(cfg)))
        .header("x-goog-api-key", &cfg.api_key)
        .header("X-Goog-Upload-Protocol", "resumable")
        .header("X-Goog-Upload-Command", "start")
        .header("X-Goog-Upload-Header-Content-Length", bytes.len().to_string())
        .header("X-Goog-Upload-Header-Content-Type", mime)
        .json(&json!({"file": {"display_name": display_name, "mime_type": mime}}))
        .send()
        .await?;
    let start = check(start).await?;
    let upload_url = start
        .headers()
        .get("x-goog-upload-url")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow!("Gemini upload: missing upload URL"))?
        .to_string();

    let resp = client
        .post(upload_url)
        .header("X-Goog-Upload-Offset", "0")
        .header("X-Goog-Upload-Command", "upload, finalize")
        .body(bytes)
        .send()
        .await?;
    let body: Value = check(resp).await?.json().await?;
    let f = &body["file"];
    let name = f["name"].as_str().context("Gemini upload: missing file name")?.to_string();
    let uri = f["uri"].as_str().context("Gemini upload: missing file uri")?.to_string();

    // Wait until the file is ACTIVE (audio is usually immediate).
    let mut state = f["state"].as_str().unwrap_or("ACTIVE").to_string();
    for _ in 0..30 {
        if state != "PROCESSING" {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        let r = client
            .get(format!("{}/v1beta/{}", base(cfg), name))
            .header("x-goog-api-key", &cfg.api_key)
            .send()
            .await?;
        let v: Value = check(r).await?.json().await?;
        state = v["state"].as_str().unwrap_or("ACTIVE").to_string();
    }
    if state == "FAILED" {
        bail!("Gemini failed to process the uploaded audio");
    }
    Ok(UploadedFile { name, uri })
}

async fn interact(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    req: &Request<'_>,
    file: &UploadedFile,
) -> Result<Transcription> {
    let mut transcription = json!({
        "mode": {
            "type": "verbatim",
            "diarization_mode": "speaker",
            "timestamp_granularities": ["word"]
        }
    });
    if let Some(lang) = req.language {
        transcription["language_codes"] = json!([lang]);
    }
    let body = json!({
        "model": cfg.model,
        "input": [{"type": "audio", "uri": file.uri, "mime_type": MIME}],
        "generation_config": {"transcription_config": transcription}
    });
    let resp = client
        .post(format!("{}/v1beta/interactions", base(cfg)))
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(Duration::from_secs(1500))
        .json(&body)
        .send()
        .await?;
    let mut v: Value = check(resp).await?.json().await?;

    // Poll if the interaction is still running.
    for _ in 0..300 {
        match v["status"].as_str() {
            Some("in_progress") | Some("queued") | Some("pending") => {}
            Some("failed") | Some("cancelled") => bail!("Gemini interaction {}: {}", v["status"], v["error"]),
            _ => break,
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let id = v["id"].as_str().unwrap_or_default();
        let path = if id.starts_with("interactions/") { id.to_string() } else { format!("interactions/{id}") };
        let r = client
            .get(format!("{}/v1beta/{}", base(cfg), path))
            .header("x-goog-api-key", &cfg.api_key)
            .send()
            .await?;
        v = check(r).await?.json().await?;
    }
    Ok(Transcription { segments: parse_interaction(&v, req.duration), usage: extract_usage(&v), language: extract_language(&v), raw: Some(v), fallback: None })
}

#[derive(Debug)]
struct Word {
    text: String,
    speaker: Option<String>,
    start: f64,
    end: f64,
}

fn collect_words(v: &Value, out: &mut Vec<Word>) {
    match v {
        Value::Object(m) => {
            if m.get("type").and_then(Value::as_str) == Some("word_info") {
                if let (Some(text), Some(start), Some(end)) = (
                    m.get("text").and_then(Value::as_str),
                    m.get("start_offset").and_then(parse_time),
                    m.get("end_offset").and_then(parse_time),
                ) {
                    out.push(Word {
                        text: text.to_string(),
                        speaker: m
                            .get("speaker")
                            .and_then(Value::as_str)
                            .filter(|s| !s.trim().is_empty())
                            .map(speaker_label),
                        start,
                        end,
                    });
                }
                return;
            }
            m.values().for_each(|c| collect_words(c, out));
        }
        Value::Array(a) => a.iter().for_each(|c| collect_words(c, out)),
        _ => {}
    }
}

fn collect_text(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            if m.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(t) = m.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
                return;
            }
            m.values().for_each(|c| collect_text(c, out));
        }
        Value::Array(a) => a.iter().for_each(|c| collect_text(c, out)),
        _ => {}
    }
}

/// "spk_1" → "Speaker 1"
fn speaker_label(raw: &str) -> String {
    let n = raw.trim_start_matches(|c: char| !c.is_ascii_digit());
    if n.is_empty() { raw.to_string() } else { format!("Speaker {n}") }
}

fn parse_interaction(v: &Value, duration: f64) -> Vec<Segment> {
    let mut words = Vec::new();
    collect_words(v, &mut words);
    if words.is_empty() {
        let mut text = String::new();
        collect_text(v, &mut text);
        if text.trim().is_empty() {
            return vec![];
        }
        return vec![Segment { start: 0.0, end: duration, text, speaker: None }];
    }
    group_words(words)
}

/// Group words into readable segments: split on speaker change, long pauses,
/// or at sentence ends once a segment gets long.
fn group_words(mut words: Vec<Word>) -> Vec<Segment> {
    // Repair impossible word timings (seen on long audio: one word "lasting" most of the chunk).
    for w in &mut words {
        if w.end < w.start || w.end - w.start > MAX_WORD_SECS {
            w.end = w.start + 1.0;
        }
    }
    let mut out: Vec<Segment> = Vec::new();
    let mut cur: Option<Segment> = None;
    for w in words {
        if let Some(c) = cur.as_mut() {
            let len = c.end - c.start;
            let sentence_end = c.text.ends_with(['.', '?', '!']);
            let split = c.speaker != w.speaker
                || w.start - c.end > 1.2
                || (len > 15.0 && sentence_end)
                || len > 40.0;
            if !split {
                if !w.text.starts_with([',', '.', '!', '?', ';', ':']) && !c.text.is_empty() {
                    c.text.push(' ');
                }
                c.text.push_str(&w.text);
                c.end = w.end;
                continue;
            }
            out.push(cur.take().unwrap());
        }
        cur = Some(Segment { start: w.start, end: w.end, text: w.text, speaker: w.speaker });
    }
    out.extend(cur);
    out
}

// ---------------------------------------------------------------------------
// generateContent fallback (general Gemini models)
// ---------------------------------------------------------------------------

/// The `generateContent` body used to transcribe one audio chunk with a general Gemini model
/// (also used for batch requests).
pub fn transcription_request(audio: &[u8], language: Option<&str>, known_speakers: &[String], previous_text: &str) -> Value {
    let data = base64::engine::general_purpose::STANDARD.encode(audio);
    let mut prompt = String::from(
        "Transcribe this audio verbatim with speaker diarization. Label speakers consistently as \
         \"Speaker 1\", \"Speaker 2\", etc. Return segments of one to a few sentences with start and \
         end timestamps in seconds from the beginning of this audio.",
    );
    if let Some(lang) = language {
        prompt.push_str(&format!(" The audio language is {lang}."));
    }
    if !known_speakers.is_empty() {
        prompt.push_str(&format!(
            " This audio continues an earlier recording. Speakers identified so far: {}. Reuse these labels \
             for the same voices. The previous part ended with: \"{}\"",
            known_speakers.join(", "),
            previous_text.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect::<String>()
        ));
    }
    json!({
        "contents": [{"parts": [
            {"inline_data": {"mime_type": MIME, "data": data}},
            {"text": prompt}
        ]}],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "ARRAY",
                "items": {
                    "type": "OBJECT",
                    "properties": {
                        "start": {"type": "NUMBER"},
                        "end": {"type": "NUMBER"},
                        "speaker": {"type": "STRING"},
                        "text": {"type": "STRING"}
                    },
                    "required": ["start", "end", "speaker", "text"]
                }
            }
        }
    })
}

/// Re-does parts Google blocks on another general model. Google says only "OTHER", and its
/// filter tripped on ordinary meeting audio with 3.7 and 3.8 Flash where 3.5 Flash didn't.
pub const BLOCK_FALLBACK_MODEL: &str = "gemini-3.5-flash";
/// Re-does parts 3.5 Flash blocks too. The speech model hasn't blocked anything so far.
pub const LAST_RESORT_MODEL: &str = "gemini-3.5-transcribe";

/// The model to re-do a blocked part with, if any.
pub fn block_fallback(cfg: &ProviderConfig) -> Option<&'static str> {
    if cfg.kind != ProviderKind::Gemini || cfg.model.contains("transcribe") {
        return None;
    }
    Some(if cfg.model == BLOCK_FALLBACK_MODEL { LAST_RESORT_MODEL } else { BLOCK_FALLBACK_MODEL })
}

/// Google refused to transcribe the audio. The request is billed anyway (input + thinking).
#[derive(Debug)]
pub struct Blocked {
    pub reason: String,
    pub usage: TokenUsage,
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "blocked by Google ({})", self.reason)
    }
}

impl std::error::Error for Blocked {}

/// Finish reasons that mean Google withheld the answer rather than the audio having no speech.
const BLOCK_FINISH_REASONS: &[&str] = &["SAFETY", "PROHIBITED_CONTENT", "BLOCKLIST", "SPII", "RECITATION"];

/// Segments + usage from a `generateContent` response (or a batch result line's `response`).
pub fn parse_generate_response(v: &Value) -> Result<Transcription> {
    let blocked = |reason: &str| anyhow::Error::new(Blocked { reason: reason.to_string(), usage: extract_usage(v) });
    let Some(candidate) = v.get("candidates").and_then(|c| c.get(0)) else {
        let feedback = v.get("promptFeedback").or_else(|| v.get("prompt_feedback"));
        let reason = feedback.and_then(|f| f.get("blockReason").or_else(|| f.get("block_reason"))).and_then(Value::as_str);
        match reason {
            Some(r) => return Err(blocked(r)),
            None => bail!("Gemini returned no result"),
        }
    };
    let text: String = candidate["content"]["parts"]
        .as_array()
        .map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect())
        .unwrap_or_default();
    if text.trim().is_empty() {
        let reason = candidate["finishReason"].as_str().or(candidate["finish_reason"].as_str()).unwrap_or("unknown");
        if BLOCK_FINISH_REASONS.contains(&reason) {
            return Err(blocked(reason));
        }
        bail!("Gemini returned no text (finish reason: {reason})");
    }
    Ok(Transcription {
        segments: parse_json_segments(&text)?,
        usage: extract_usage(v),
        language: None,
        raw: Some(v.clone()),
        fallback: None,
    })
}

async fn generate_content(client: &reqwest::Client, cfg: &ProviderConfig, req: &Request<'_>) -> Result<Transcription> {
    let bytes = tokio::fs::read(req.path).await?;
    let body = transcription_request(&bytes, req.language, req.known_speakers, req.previous_text);
    let resp = client
        .post(format!("{}/v1beta/models/{}:generateContent", base(cfg), cfg.model))
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(Duration::from_secs(1500))
        .json(&body)
        .send()
        .await?;
    let v: Value = check(resp).await?.json().await?;
    parse_generate_response(&v)
}

pub fn parse_json_segments(text: &str) -> Result<Vec<Segment>> {
    let cleaned = text.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```");
    let arr: Vec<Value> = serde_json::from_str(cleaned.trim()).context("Gemini returned invalid JSON")?;
    Ok(arr
        .iter()
        .filter_map(|s| {
            Some(Segment {
                start: parse_time(s.get("start")?)?,
                end: parse_time(s.get("end")?)?,
                text: s.get("text")?.as_str()?.to_string(),
                speaker: s.get("speaker").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_interaction_words_into_segments() {
        let v = json!({
            "id": "interactions/abc", "status": "completed",
            "steps": [{"type": "model_output", "content": [{"type": "text", "text": "Hello world. Hi.",
                "annotations": [
                    {"type": "word_info", "text": "Hello", "speaker": "spk_1", "start_offset": "0.100s", "end_offset": "0.450s"},
                    {"type": "word_info", "text": "world.", "speaker": "spk_1", "start_offset": "0.500s", "end_offset": "0.850s"},
                    {"type": "word_info", "text": "Hi.", "speaker": "spk_2", "start_offset": "1.000s", "end_offset": "1.300s"}
                ]}]}]
        });
        let s = parse_interaction(&v, 10.0);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "Hello world.");
        assert_eq!(s[0].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!((s[0].start, s[0].end), (0.1, 0.85));
        assert_eq!(s[1].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn repairs_bogus_word_timings_and_empty_speakers() {
        let v = json!({"steps": [{"content": [{"type": "text", "text": "a b", "annotations": [
            {"type": "word_info", "text": "Hello", "speaker": "", "start_offset": "1.0s", "end_offset": "1400.0s"},
            {"type": "word_info", "text": "there.", "speaker": "", "start_offset": "1.5s", "end_offset": "1.9s"}
        ]}]}]});
        let s = parse_interaction(&v, 1500.0);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].start, s[0].end, s[0].speaker.clone()), (1.0, 1.9, None));
    }

    #[test]
    fn falls_back_to_text() {
        let v = json!({"steps": [{"content": [{"type": "text", "text": "just text"}]}]});
        assert_eq!(parse_interaction(&v, 3.0)[0].text, "just text");
    }

    #[test]
    fn parses_fenced_json() {
        let s = parse_json_segments("```json\n[{\"start\":\"00:01.5\",\"end\":3,\"speaker\":\"Speaker 1\",\"text\":\"hi\"}]\n```").unwrap();
        assert_eq!(s[0].start, 1.5);
    }

    #[test]
    fn reports_blocks_with_their_usage() {
        let e = parse_generate_response(&json!({"promptFeedback": {"blockReason": "OTHER"},
            "usageMetadata": {"promptTokenCount": 7279, "thoughtsTokenCount": 1566}}))
        .unwrap_err();
        let b = e.downcast_ref::<Blocked>().unwrap();
        assert_eq!((b.reason.as_str(), b.usage.input_tokens, b.usage.output_tokens), ("OTHER", 7279, 1566));
        assert_eq!(e.to_string(), "blocked by Google (OTHER)");
        // A withheld answer is a block; an empty one isn't.
        let safety = json!({"candidates": [{"content": {"parts": []}, "finishReason": "SAFETY"}]});
        assert!(parse_generate_response(&safety).unwrap_err().downcast_ref::<Blocked>().is_some());
        let empty = json!({"candidates": [{"content": {"parts": []}, "finishReason": "STOP"}]});
        assert!(parse_generate_response(&empty).unwrap_err().downcast_ref::<Blocked>().is_none());
    }

    #[test]
    fn only_general_models_fall_back() {
        let cfg = |kind: ProviderKind, model: &str| ProviderConfig {
            id: "g".into(), name: "G".into(), kind, base_url: String::new(), api_key: String::new(), model: model.into(),
            price_per_minute: None, price_input_per_mtok: None, price_output_per_mtok: None,
        };
        assert_eq!(block_fallback(&cfg(ProviderKind::Gemini, "gemini-3.8-flash")), Some(BLOCK_FALLBACK_MODEL));
        assert_eq!(block_fallback(&cfg(ProviderKind::Gemini, BLOCK_FALLBACK_MODEL)), Some(LAST_RESORT_MODEL));
        assert_eq!(block_fallback(&cfg(ProviderKind::Gemini, LAST_RESORT_MODEL)), None);
        assert_eq!(block_fallback(&cfg(ProviderKind::Openai, "whisper-1")), None);
    }

    /// Against a local fake that blocks every model but the fallback: `python3 scripts/mock_gemini_batch.py`
    /// then `MOCK_BATCH_URL=http://127.0.0.1:8766 cargo test falls_back_on_block -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn falls_back_on_block_against_mock() {
        let cfg = ProviderConfig {
            id: "gemini".into(), name: "Gemini".into(), kind: ProviderKind::Gemini,
            base_url: std::env::var("MOCK_BATCH_URL").unwrap(), api_key: "test-key".into(), model: "gemini-3.8-flash".into(),
            price_per_minute: None, price_input_per_mtok: None, price_output_per_mtok: None,
        };
        let path = std::env::temp_dir().join("fallback-test.ogg");
        std::fs::write(&path, b"fake-audio").unwrap();
        let messages = std::sync::Mutex::new(Vec::new());
        let on_retry = |m: String| messages.lock().unwrap().push(m);
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let req = Request { path: &path, duration: 60.0, language: Some("fil"), known_speakers: &[], previous_text: "", on_retry: &on_retry, cancel: &cancel };
        let client = reqwest::Client::new();
        let at = |prefix: &str, model: &str| ProviderConfig { base_url: format!("{}{prefix}", cfg.base_url), model: model.into(), ..cfg.clone() };
        let blocked_on = |f: &crate::providers::Fallback| f.blocked.iter().map(|(m, u)| (m.clone(), u.input_tokens)).collect::<Vec<_>>();

        // 3.8 Flash blocked → 3.5 Flash.
        let t = super::super::transcribe(&client, &cfg, &req).await.unwrap();
        assert_eq!(t.segments[0].text, format!("text from {BLOCK_FALLBACK_MODEL}"));
        let f = t.fallback.unwrap();
        assert_eq!((f.model.as_str(), blocked_on(&f)), (BLOCK_FALLBACK_MODEL, vec![("gemini-3.8-flash".into(), 1500)]));
        assert_eq!(f.blocked[0].1.output_tokens, 400);
        assert!(messages.lock().unwrap()[0].contains("blocked this part on gemini-3.8-flash (OTHER) — re-doing it with gemini-3.5-flash"));

        // 3.5 Flash blocked too → 3.5 Transcribe.
        let t = super::super::transcribe(&client, &at("/flash-blocks", "gemini-3.8-flash"), &req).await.unwrap();
        assert_eq!((t.segments[0].text.as_str(), t.segments[0].speaker.as_deref()), ("text from gemini-3.5-transcribe", Some("Speaker 1")));
        assert_eq!(t.usage.input_tokens, 1200);
        let f = t.fallback.unwrap();
        assert_eq!(f.model, LAST_RESORT_MODEL);
        assert_eq!(blocked_on(&f), vec![("gemini-3.8-flash".into(), 1500), (BLOCK_FALLBACK_MODEL.into(), 1500)]);
        assert_eq!(f.blocked_models(), "gemini-3.8-flash and gemini-3.5-flash");

        // Everything fails: the error says which models were tried, and nothing after 3.5 Transcribe.
        let e = super::super::transcribe(&client, &at("/always-block", "gemini-3.8-flash"), &req).await.unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.starts_with("Google blocked this part on gemini-3.8-flash and gemini-3.5-flash, then re-doing it with gemini-3.5-transcribe failed: "), "{msg}");
        // Starting on the last resort: its own error, nothing else tried.
        let e = super::super::transcribe(&client, &at("/always-block", LAST_RESORT_MODEL), &req).await.unwrap_err();
        assert!(!format!("{e:#}").starts_with("Google blocked"));
        println!("fallback ok: {:?}", messages.lock().unwrap());
    }
}
