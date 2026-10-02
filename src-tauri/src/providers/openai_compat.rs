//! OpenAI-compatible `/audio/transcriptions` (OpenAI, Groq, local servers, ...).

use super::{check, extract_usage, parse_time, Request, Segment, Transcription};
use crate::settings::ProviderConfig;
use anyhow::{Context, Result};
use reqwest::multipart::{Form, Part};
use serde_json::Value;

pub async fn transcribe(client: &reqwest::Client, cfg: &ProviderConfig, req: &Request<'_>) -> Result<Transcription> {
    let bytes = tokio::fs::read(req.path).await?;
    let file = Part::bytes(bytes).file_name("audio.ogg").mime_str("audio/ogg")?;
    // `gpt-4o-transcribe-diarize` returns speaker-labelled segments; `gpt-4o-*` models
    // only return plain text; Whisper models return timestamped segments.
    let diarize = cfg.model.contains("diarize");
    let plain = !diarize && cfg.model.starts_with("gpt-4o");
    let mut form = Form::new().part("file", file).text("model", cfg.model.clone());
    form = if diarize {
        form.text("response_format", "diarized_json").text("chunking_strategy", "auto")
    } else if plain {
        form.text("response_format", "json")
    } else {
        form.text("response_format", "verbose_json").text("timestamp_granularities[]", "segment")
    };
    if let Some(lang) = req.language {
        form = form.text("language", lang.to_string());
    }
    if !plain && !req.previous_text.is_empty() && !diarize {
        // Whisper uses the prompt as preceding context, which helps continuity between chunks.
        let tail: String = req.previous_text.chars().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect();
        form = form.text("prompt", tail);
    }

    let url = format!("{}/audio/transcriptions", cfg.base_url.trim_end_matches('/'));
    let resp = client.post(url).bearer_auth(&cfg.api_key).multipart(form).send().await?;
    let body: Value = check(resp).await?.json().await.context("invalid JSON from provider")?;
    Ok(Transcription { segments: parse(&body, req.duration), usage: extract_usage(&body) })
}

fn parse(body: &Value, duration: f64) -> Vec<Segment> {
    if let Some(segs) = body.get("segments").and_then(Value::as_array) {
        let out: Vec<Segment> = segs
            .iter()
            .filter_map(|s| {
                Some(Segment {
                    start: parse_time(s.get("start")?)?,
                    end: parse_time(s.get("end")?)?,
                    text: s.get("text")?.as_str()?.to_string(),
                    speaker: s.get("speaker").and_then(Value::as_str).map(|v| format!("Speaker {v}")),
                })
            })
            .collect();
        if !out.is_empty() {
            return out;
        }
    }
    match body.get("text").and_then(Value::as_str) {
        Some(t) if !t.trim().is_empty() => {
            vec![Segment { start: 0.0, end: duration, text: t.to_string(), speaker: None }]
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_verbose_json() {
        let body = json!({"text": "hi there", "segments": [
            {"id": 0, "start": 0.0, "end": 1.2, "text": " hi"},
            {"id": 1, "start": 1.2, "end": 2.0, "text": " there"}
        ]});
        let s = parse(&body, 5.0);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].start, 1.2);
        assert_eq!(s[0].speaker, None);
    }

    #[test]
    fn parses_diarized_and_plain() {
        let d = parse(&json!({"segments": [{"speaker": "A", "start": 0.5, "end": 1.0, "text": "yo"}]}), 5.0);
        assert_eq!(d[0].speaker.as_deref(), Some("Speaker A"));
        let p = parse(&json!({"text": "hello"}), 5.0);
        assert_eq!(p, vec![Segment { start: 0.0, end: 5.0, text: "hello".into(), speaker: None }]);
    }
}
