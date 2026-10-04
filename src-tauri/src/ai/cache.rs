//! Tier 3: Gemini explicit context caching. A whole transcript (plus the system prompt) is
//! uploaded once as a `cachedContents` object with a TTL; questions then reference it and pay
//! the cached-input rate. Anthropic uses `cache_control` on the system block instead (chat.rs).

use super::store::{self, ContextCache};
use super::AiCtx;
use crate::providers::{check, HttpError};
use crate::settings::{ProviderConfig, ProviderKind};
use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;

/// Returns the cache name to reference, or None when caching isn't possible (not Gemini,
/// content too small for the model's minimum, API refused). Never fails the question.
pub async fn ensure_gemini_cache(ctx: &AiCtx<'_>, cfg: &ProviderConfig, transcript_id: &str, hash: &str, system: &str, content: &str) -> Option<String> {
    if cfg.kind != ProviderKind::Gemini || !ctx.settings.ai_context_cache {
        return None;
    }
    let existing = store::context_cache_get(&ctx.db(), transcript_id, &cfg.model).ok().flatten();
    if let Some(cc) = &existing {
        let fresh = chrono::DateTime::parse_from_rfc3339(&cc.expires_at)
            .map(|t| t > chrono::Local::now() + chrono::Duration::seconds(90))
            .unwrap_or(false);
        if cc.content_hash == hash && fresh {
            return Some(cc.cache_name.clone());
        }
        // Stale or expired: drop it (best effort) and make a new one.
        let _ = ctx.http.delete(format!("{}/v1beta/{}", cfg.base_url.trim_end_matches('/'), cc.cache_name)).header("x-goog-api-key", &cfg.api_key).send().await;
        let _ = store::context_cache_delete(&ctx.db(), transcript_id, &cfg.model);
    }
    match create(ctx, cfg, transcript_id, hash, system, content).await {
        Ok(Some(cc)) => {
            let _ = store::context_cache_put(&ctx.db(), transcript_id, &cfg.model, &cc);
            Some(cc.cache_name)
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("context cache for {transcript_id}: {e:#}");
            None
        }
    }
}

async fn create(ctx: &AiCtx<'_>, cfg: &ProviderConfig, transcript_id: &str, hash: &str, system: &str, content: &str) -> Result<Option<ContextCache>> {
    let ttl = ctx.settings.ai_context_cache_ttl_minutes.max(5) as u64 * 60;
    let body = json!({
        "model": format!("models/{}", cfg.model),
        "displayName": format!("transcriber:{transcript_id}"),
        "systemInstruction": { "parts": [{ "text": system }] },
        "contents": [{ "role": "user", "parts": [{ "text": content }] }],
        "ttl": format!("{ttl}s"),
    });
    let resp = ctx
        .http
        .post(format!("{}/v1beta/cachedContents", cfg.base_url.trim_end_matches('/')))
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(Duration::from_secs(300))
        .json(&body)
        .send()
        .await?;
    let v: Value = match check(resp).await {
        Ok(r) => r.json().await?,
        // 400 = below the model's minimum cache size (or unsupported model): fall back silently.
        Err(e) if e.downcast_ref::<HttpError>().is_some_and(|h| h.status == 400) => return Ok(None),
        Err(e) => return Err(e),
    };
    let Some(name) = v["name"].as_str() else { return Ok(None) };
    let expires = v["expireTime"].as_str().map(str::to_string).unwrap_or_else(|| (chrono::Local::now() + chrono::Duration::seconds(ttl as i64)).to_rfc3339());
    Ok(Some(ContextCache {
        cache_name: name.to_string(),
        content_hash: hash.to_string(),
        expires_at: expires,
        tokens: v["usageMetadata"]["totalTokenCount"].as_i64().unwrap_or(0),
    }))
}
