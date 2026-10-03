//! Cost accounting for AI requests, written to the same `usage` table as transcriptions.

use crate::db;
use crate::providers::TokenUsage;
use crate::settings::ProviderConfig;
use rusqlite::Connection;

/// USD for `usage` at the provider's text prices (0 when no price is known).
pub fn cost(cfg: &ProviderConfig, u: &TokenUsage) -> f64 {
    let Some(p) = cfg.text_price() else { return 0.0 };
    let uncached = u.input_tokens.saturating_sub(u.cached_tokens) as f64;
    (uncached * p.input + u.cached_tokens as f64 * p.cached_input + u.output_tokens as f64 * p.output) / 1e6
}

pub struct Record<'a> {
    /// 'ai' | 'embed'
    pub kind: &'a str,
    pub cfg: &'a ProviderConfig,
    pub usage: TokenUsage,
    pub requests: i64,
    pub status: &'a str,
    pub transcript_id: Option<&'a str>,
    pub title: &'a str,
}

/// Insert a usage row; returns the cost. Errors are logged, not propagated (bookkeeping must
/// never fail the user's request).
pub fn record(c: &Connection, r: &Record) -> f64 {
    let cost_usd = cost(r.cfg, &r.usage);
    if r.requests == 0 && r.usage.input_tokens == 0 {
        return 0.0;
    }
    let row = db::NewUsage {
        kind: r.kind,
        transcript_id: r.transcript_id,
        title: r.title,
        provider: &r.cfg.id,
        provider_name: &r.cfg.name,
        model: &r.cfg.model,
        status: r.status,
        original_seconds: 0.0,
        sent_seconds: 0.0,
        requests: r.requests,
        input_tokens: r.usage.input_tokens as i64,
        output_tokens: r.usage.output_tokens as i64,
        price_per_minute: 0.0,
        cost_usd,
        saved_usd: 0.0,
    };
    if let Err(e) = db::insert_usage(c, &row) {
        eprintln!("usage row ({}): {e:#}", r.kind);
    }
    cost_usd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_tokens_cost_less() {
        let cfg = crate::settings::anthropic_provider();
        let u = TokenUsage { input_tokens: 1_000_000, output_tokens: 0, cached_tokens: 500_000 };
        // Opus 5.5: $4 uncached, $0.20 cached per 1M.
        assert!((cost(&cfg, &u) - (2.0 + 0.10)).abs() < 1e-9);
    }
}
