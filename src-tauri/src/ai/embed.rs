//! Text embeddings for semantic search: Gemini `batchEmbedContents` or OpenAI-compatible
//! `/embeddings`. Vectors are unit-normalised and stored as little-endian f32 BLOBs.

use crate::providers::retry::{with_retry, RetryCtx, RetryPolicy};
use crate::providers::{check, extract_usage, TokenUsage};
use crate::settings::{ProviderConfig, ProviderKind};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Smaller than the models' defaults; plenty for a personal library and keeps the DB small.
pub const DIMENSIONS: usize = 768;
/// Gemini accepts up to 100 texts per batch request.
const BATCH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmbedTask {
    Document,
    Query,
}

pub struct EmbedCall<'a> {
    pub cancel: &'a AtomicBool,
    pub on_retry: &'a (dyn Fn(String) + Send + Sync),
}

/// Embed `texts` (in order). Usage is reported when the API gives it, else estimated.
pub async fn embed(http: &reqwest::Client, cfg: &ProviderConfig, texts: &[String], task: EmbedTask, call: &EmbedCall<'_>) -> Result<(Vec<Vec<f32>>, TokenUsage)> {
    if texts.is_empty() {
        return Ok((vec![], TokenUsage::default()));
    }
    if cfg.api_key.trim().is_empty() {
        bail!("No API key set for {}. Add one in Settings.", cfg.name);
    }
    let policy = RetryPolicy::interactive();
    let ctx = RetryCtx { cancel: call.cancel, on_retry: call.on_retry, provider_name: &cfg.name };
    let mut out = Vec::with_capacity(texts.len());
    let mut usage = TokenUsage::default();
    for batch in texts.chunks(BATCH) {
        let (vecs, u) = with_retry(&policy, &ctx, || async move {
            match cfg.kind {
                ProviderKind::Gemini => gemini(http, cfg, batch, task).await,
                ProviderKind::Openai => openai(http, cfg, batch).await,
                ProviderKind::Anthropic => bail!("Anthropic has no embeddings API; pick Gemini or an OpenAI-compatible provider."),
            }
        })
        .await
        .with_context(|| format!("embedding with {} ({})", cfg.name, cfg.model))?;
        if vecs.len() != batch.len() {
            bail!("{} returned {} embeddings for {} texts", cfg.name, vecs.len(), batch.len());
        }
        out.extend(vecs.into_iter().map(normalise));
        usage += u;
    }
    Ok((out, usage))
}

async fn gemini(http: &reqwest::Client, cfg: &ProviderConfig, texts: &[String], task: EmbedTask) -> Result<(Vec<Vec<f32>>, TokenUsage)> {
    let model = format!("models/{}", cfg.model.trim_start_matches("models/"));
    let requests: Vec<Value> = texts
        .iter()
        .map(|t| {
            json!({
                "model": model,
                "content": { "parts": [{ "text": t }] },
                "taskType": if task == EmbedTask::Query { "RETRIEVAL_QUERY" } else { "RETRIEVAL_DOCUMENT" },
                "outputDimensionality": DIMENSIONS,
            })
        })
        .collect();
    let url = format!("{}/v1beta/{model}:batchEmbedContents", cfg.base_url.trim_end_matches('/'));
    let resp = http
        .post(url)
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(Duration::from_secs(120))
        .json(&json!({ "requests": requests }))
        .send()
        .await?;
    let v: Value = check(resp).await?.json().await?;
    let vecs = v["embeddings"]
        .as_array()
        .map(|a| a.iter().map(|e| values(&e["values"])).collect::<Vec<_>>())
        .unwrap_or_default();
    // batchEmbedContents reports no usage; estimate for the cost table.
    let est: usize = texts.iter().map(|t| super::estimate_tokens(t)).sum();
    Ok((vecs, TokenUsage { input_tokens: est as u64, output_tokens: 0, cached_tokens: 0 }))
}

async fn openai(http: &reqwest::Client, cfg: &ProviderConfig, texts: &[String]) -> Result<(Vec<Vec<f32>>, TokenUsage)> {
    let mut body = json!({ "model": cfg.model, "input": texts });
    if cfg.model.contains("text-embedding-3") {
        body["dimensions"] = json!(DIMENSIONS);
    }
    let url = format!("{}/embeddings", cfg.base_url.trim_end_matches('/'));
    let resp = http.post(url).bearer_auth(&cfg.api_key).timeout(Duration::from_secs(120)).json(&body).send().await?;
    let v: Value = check(resp).await?.json().await?;
    let mut rows: Vec<(usize, Vec<f32>)> = v["data"]
        .as_array()
        .map(|a| a.iter().enumerate().map(|(i, e)| (e["index"].as_u64().map(|x| x as usize).unwrap_or(i), values(&e["embedding"]))).collect())
        .unwrap_or_default();
    rows.sort_by_key(|(i, _)| *i);
    Ok((rows.into_iter().map(|(_, v)| v).collect(), extract_usage(&v)))
}

fn values(v: &Value) -> Vec<f32> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()).unwrap_or_default()
}

pub fn normalise(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Cosine similarity of unit vectors.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_round_trip_and_normalise() {
        let v = normalise(vec![3.0, 4.0]);
        assert!((dot(&v, &v) - 1.0).abs() < 1e-6);
        assert_eq!(from_blob(&to_blob(&v)), v);
    }
}
