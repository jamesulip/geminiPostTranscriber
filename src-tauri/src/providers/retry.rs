//! Retry loop shared by transcription and AI requests: honours `Retry-After`, backs off
//! exponentially otherwise, and races every wait against the Cancel flag.

use super::HttpError;
use anyhow::{bail, Result};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct RetryPolicy {
    /// Attempts for server/network errors.
    pub max_attempts: u32,
    /// Rate limits are expected on long jobs; keep waiting as long as the provider asks,
    /// up to this much total waiting per request.
    pub rate_limit_budget_secs: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy { max_attempts: 6, rate_limit_budget_secs: 45 * 60 }
    }
}

impl RetryPolicy {
    /// Interactive requests (chat): fewer attempts, give up on rate limits after a few minutes.
    pub fn interactive() -> Self {
        RetryPolicy { max_attempts: 4, rate_limit_budget_secs: 3 * 60 }
    }
}

pub struct RetryCtx<'a> {
    /// Set by the Cancel button; checked during requests and waits.
    pub cancel: &'a AtomicBool,
    /// Called with a status message when waiting before a retry.
    pub on_retry: &'a (dyn Fn(String) + Send + Sync),
    pub provider_name: &'a str,
}

/// Resolves once `flag` is set (polled; cheap).
pub async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub fn is_retryable(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<HttpError>() {
        Some(h) => (h.status == 429 && !h.daily) || h.status >= 500,
        None => e.downcast_ref::<reqwest::Error>().is_some_and(|r| r.is_timeout() || r.is_connect()),
    }
}

/// Runs `op` until it succeeds, fails with a non-retryable error, or the budget is spent.
/// An in-flight request is dropped (cancelled) as soon as the flag is set.
pub async fn with_retry<T, F, Fut>(policy: &RetryPolicy, ctx: &RetryCtx<'_>, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0u32;
    let mut waited = 0u64;
    loop {
        if ctx.cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let res = tokio::select! {
            r = op() => r,
            _ = cancelled(ctx.cancel) => bail!("Cancelled"),
        };
        let e = match res {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        let http = e.downcast_ref::<HttpError>();
        let rate_limited = http.is_some_and(|h| h.status == 429 && !h.daily);
        let retry = if rate_limited { waited < policy.rate_limit_budget_secs } else { attempt + 1 < policy.max_attempts && is_retryable(&e) };
        if !retry {
            return Err(e);
        }
        attempt += 1;
        // Honour the provider's delay (Retry-After / RetryInfo), else back off exponentially.
        let hinted = http.and_then(|h| h.retry_after);
        let wait = hinted.unwrap_or(2u64.pow(attempt.min(8))).clamp(1, 300);
        waited += wait;
        // Outermost context names the step, e.g. "uploading audio" / "transcribing".
        let step = if e.chain().count() > 1 { format!(" while {e}") } else { String::new() };
        let name = ctx.provider_name;
        let why = match http {
            Some(h) if h.status == 429 => match &h.quota {
                Some(q) => format!("{name} quota reached{step}: {q}"),
                None => format!("Rate limited by {name}{step}"),
            },
            Some(h) => format!("{name} returned HTTP {}", h.status),
            None => format!("Network error talking to {name}"),
        };
        let tries = if rate_limited {
            format!("waited {}m so far", waited / 60)
        } else {
            format!("attempt {}/{}", attempt + 1, policy.max_attempts)
        };
        (ctx.on_retry)(format!("{why} — retrying in {wait}s ({tries})"));
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
            _ = cancelled(ctx.cancel) => bail!("Cancelled"),
        }
    }
}
