//! Gemini. `*-transcribe` models use the Interactions API (native diarization and word
//! timestamps); other models fall back to `generateContent` with a JSON schema.

use super::{check, extract_language, extract_usage, parse_time, Request, Segment, Transcription};
use crate::settings::ProviderConfig;
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
    Ok(Transcription { segments: parse_interaction(&v, req.duration), usage: extract_usage(&v), language: extract_language(&v), raw: Some(v) })
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

/// Segments + usage from a `generateContent` response (or a batch result line's `response`).
pub fn parse_generate_response(v: &Value) -> Result<Transcription> {
    let Some(candidate) = v.get("candidates").and_then(|c| c.get(0)) else {
        let feedback = v.get("promptFeedback").or_else(|| v.get("prompt_feedback"));
        let reason = feedback.and_then(|f| f.get("blockReason").or_else(|| f.get("block_reason"))).and_then(Value::as_str);
        match reason {
            Some(r) => bail!("blocked by Google ({r})"),
            None => bail!("Gemini returned no result"),
        }
    };
    let text: String = candidate["content"]["parts"]
        .as_array()
        .map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect())
        .unwrap_or_default();
    if text.trim().is_empty() {
        let reason = candidate["finishReason"].as_str().or(candidate["finish_reason"].as_str()).unwrap_or("unknown");
        bail!("Gemini returned no text (finish reason: {reason})");
    }
    Ok(Transcription { segments: parse_json_segments(&text)?, usage: extract_usage(v), language: None, raw: Some(v.clone()) })
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
}
