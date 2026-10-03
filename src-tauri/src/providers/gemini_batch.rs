//! Gemini Batch API: submit many generateContent requests at once for ~50% of the price.
//! Results usually arrive within hours (target: 24 h); jobs expire after 48 h.

use super::check;
use super::gemini::{base, parse_generate_response, upload_bytes};
use super::Transcription;
use crate::settings::ProviderConfig;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

/// One line of the batch input file.
pub fn jsonl_line(key: &str, request: &Value) -> String {
    json!({ "key": key, "request": request }).to_string()
}

/// Upload the JSONL input and create the batch job. Returns the batch name (`batches/...`).
pub async fn create(client: &reqwest::Client, cfg: &ProviderConfig, jsonl: Vec<u8>, display_name: &str) -> Result<String> {
    let file = upload_bytes(client, cfg, jsonl, "jsonl", display_name).await.context("uploading batch input")?;
    let body = json!({ "batch": { "display_name": display_name, "input_config": { "file_name": file.name } } });
    let resp = client
        .post(format!("{}/v1beta/models/{}:batchGenerateContent", base(cfg), cfg.model))
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(Duration::from_secs(300))
        .json(&body)
        .send()
        .await?;
    let v: Value = check(resp).await.context("creating batch")?.json().await?;
    find_string(&v, &|k, s| k == "name" && s.starts_with("batches/"))
        .ok_or_else(|| anyhow!("Gemini didn't return a batch name: {v}"))
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Pending,
    Running,
    Succeeded,
    Failed(String),
    Cancelled,
    Expired,
}

#[derive(Debug)]
pub struct Status {
    pub state: State,
    /// Results file (`files/...`) when finished with file output.
    pub results_file: Option<String>,
    /// Results when returned inline instead of as a file.
    pub inline: Option<Vec<Value>>,
}

/// Depth-first search for a string value whose key/value match.
fn find_string(v: &Value, pred: &dyn Fn(&str, &str) -> bool) -> Option<String> {
    match v {
        Value::Object(m) => m.iter().find_map(|(k, x)| match x {
            Value::String(s) if pred(k, s) => Some(s.clone()),
            _ => find_string(x, pred),
        }),
        Value::Array(a) => a.iter().find_map(|x| find_string(x, pred)),
        _ => None,
    }
}

fn find_array(v: &Value, keys: &[&str]) -> Option<Vec<Value>> {
    match v {
        Value::Object(m) => keys
            .iter()
            .find_map(|k| m.get(*k).and_then(|x| x.as_array()).cloned())
            .or_else(|| m.values().find_map(|x| find_array(x, keys))),
        Value::Array(a) => a.iter().find_map(|x| find_array(x, keys)),
        _ => None,
    }
}

pub fn parse_status(v: &Value) -> Status {
    let raw = find_string(v, &|k, s| k == "state" && (s.starts_with("JOB_STATE_") || s.starts_with("BATCH_STATE_")))
        .unwrap_or_default();
    let suffix = raw.trim_start_matches("JOB_STATE_").trim_start_matches("BATCH_STATE_");
    let error = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("Google reported the batch as failed")
        .to_string();
    let state = match suffix {
        "SUCCEEDED" => State::Succeeded,
        "RUNNING" => State::Running,
        "FAILED" => State::Failed(error),
        "CANCELLED" | "CANCELLING" => State::Cancelled,
        "EXPIRED" => State::Expired,
        // A finished operation without an explicit state still counts as done.
        _ if v.get("done").and_then(Value::as_bool) == Some(true) && v.get("error").is_none() => State::Succeeded,
        _ => State::Pending,
    };
    Status {
        state,
        results_file: find_string(v, &|k, s| (k == "responsesFile" || k == "responses_file") && s.starts_with("files/")),
        inline: find_array(v, &["inlinedResponses", "inlined_responses"]),
    }
}

pub async fn status(client: &reqwest::Client, cfg: &ProviderConfig, name: &str) -> Result<Status> {
    let resp = client
        .get(format!("{}/v1beta/{}", base(cfg), name))
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(Duration::from_secs(60))
        .send()
        .await?;
    Ok(parse_status(&check(resp).await.context("checking batch")?.json().await?))
}

pub async fn download(client: &reqwest::Client, cfg: &ProviderConfig, file: &str) -> Result<String> {
    let resp = client
        .get(format!("{}/download/v1beta/{}:download?alt=media", base(cfg), file))
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(Duration::from_secs(600))
        .send()
        .await?;
    Ok(check(resp).await.context("downloading batch results")?.text().await?)
}

pub async fn cancel(client: &reqwest::Client, cfg: &ProviderConfig, name: &str) -> Result<()> {
    let resp = client
        .post(format!("{}/v1beta/{}:cancel", base(cfg), name))
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(Duration::from_secs(60))
        .send()
        .await?;
    check(resp).await.context("cancelling batch")?;
    Ok(())
}

/// One result per request key: the transcription, or the error Google returned for that request.
pub fn parse_results(lines: impl Iterator<Item = Value>) -> HashMap<String, Result<Transcription, String>> {
    let mut out = HashMap::new();
    for line in lines {
        let key = line
            .get("key")
            .or_else(|| line.get("metadata").and_then(|m| m.get("key")))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let result = match line.get("response") {
            Some(r) => parse_generate_response(r).map_err(|e| e.to_string()),
            None => Err(line
                .get("error")
                .or_else(|| line.get("status"))
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("no response for this part")
                .to_string()),
        };
        out.insert(key, result);
    }
    out
}

pub fn parse_results_file(text: &str) -> HashMap<String, Result<Transcription, String>> {
    parse_results(text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_states_and_result_locations() {
        let running = json!({"name": "batches/abc", "metadata": {"state": "JOB_STATE_RUNNING"}});
        assert_eq!(parse_status(&running).state, State::Running);
        let done = json!({"name": "batches/abc", "done": true, "metadata": {"state": "BATCH_STATE_SUCCEEDED",
            "output": {"responsesFile": "files/results-1"}}});
        let s = parse_status(&done);
        assert_eq!((s.state, s.results_file.as_deref()), (State::Succeeded, Some("files/results-1")));
        let failed = json!({"state": "JOB_STATE_FAILED", "error": {"message": "quota"}});
        assert_eq!(parse_status(&failed).state, State::Failed("quota".into()));
        assert_eq!(parse_status(&json!({"state": "JOB_STATE_PENDING"})).state, State::Pending);
        assert_eq!(parse_status(&json!({"state": "JOB_STATE_EXPIRED"})).state, State::Expired);
    }

    #[test]
    fn parses_result_lines_per_key() {
        let ok = json!({"key": "chunk-0", "response": {"candidates": [{"content": {"parts": [
            {"text": "[{\"start\": 1.0, \"end\": 2.5, \"speaker\": \"Speaker 1\", \"text\": \"Magandang umaga\"}]"}]}}],
            "usageMetadata": {"promptTokenCount": 15000, "candidatesTokenCount": 400}}});
        let err = json!({"key": "chunk-1", "error": {"code": 400, "message": "audio too long"}});
        let r = parse_results_file(&format!("{ok}\n{err}\n"));
        let t = r["chunk-0"].as_ref().unwrap();
        assert_eq!((t.segments[0].start, t.usage.input_tokens, t.usage.output_tokens), (1.0, 15000, 400));
        assert_eq!(r["chunk-1"].as_ref().unwrap_err(), "audio too long");
    }

    #[test]
    fn builds_request_lines() {
        let line = jsonl_line("chunk-3", &json!({"contents": []}));
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["key"], "chunk-3");
    }

    /// Full flow against a local fake: `python3 mock_gemini_batch.py` then
    /// `MOCK_BATCH_URL=http://127.0.0.1:8766 cargo test batch_flow -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn batch_flow_against_mock() {
        let cfg = ProviderConfig {
            id: "gemini".into(), name: "Gemini".into(), kind: crate::settings::ProviderKind::Gemini,
            base_url: std::env::var("MOCK_BATCH_URL").unwrap(), api_key: "test-key".into(), model: "gemini-3.8-flash".into(),
            price_per_minute: None, price_input_per_mtok: None, price_output_per_mtok: None,
        };
        let client = reqwest::Client::new();
        let req = |k: &str| jsonl_line(k, &crate::providers::gemini::transcription_request(b"fake-audio", Some("fil"), &[], ""));
        let jsonl = format!("{}\n{}\n", req("chunk-0"), req("chunk-1"));
        let name = create(&client, &cfg, jsonl.into_bytes(), "test").await.unwrap();
        assert!(name.starts_with("batches/"));
        assert_eq!(status(&client, &cfg, &name).await.unwrap().state, State::Running);
        let s = status(&client, &cfg, &name).await.unwrap();
        assert_eq!(s.state, State::Succeeded);
        let results = parse_results_file(&download(&client, &cfg, s.results_file.as_deref().unwrap()).await.unwrap());
        assert_eq!(results.len(), 2);
        assert_eq!(results["chunk-1"].as_ref().unwrap().segments[0].text, "text of chunk-1");
        cancel(&client, &cfg, &name).await.unwrap();
        println!("batch flow ok: {name}");
    }
}

