use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Any OpenAI-compatible `/audio/transcriptions` endpoint (OpenAI, Groq, ...).
    Openai,
    Gemini,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// USD per minute of audio sent. `None` → built-in estimate for known models.
    #[serde(default)]
    pub price_per_minute: Option<f64>,
    /// USD per 1M input/output tokens. Used instead of the per-minute price when the
    /// provider reports token counts. `None` → built-in rates for known models.
    #[serde(default)]
    pub price_input_per_mtok: Option<f64>,
    #[serde(default)]
    pub price_output_per_mtok: Option<f64>,
}

/// Published token prices (USD per 1M input, 1M output) for models billed by tokens.
pub fn known_token_price(model: &str) -> Option<(f64, f64)> {
    let m = model.to_lowercase();
    // Audio input / text output rates (Oct 2026). 3.8 Flash is on introductory pricing until Dec 31, 2026.
    let table: &[(&str, (f64, f64))] = &[
        ("gemini-3.5-transcribe", (2.0, 12.0)),
        ("gemini-3.8-flash-lite", (0.50, 2.50)),
        ("gemini-3.8-flash", (0.75, 3.75)),
        // Blocked parts fall back to this one. Google lists $1.50 / $9.00 for text input; audio may cost more.
        ("gemini-3.5-flash", (1.50, 9.00)),
        ("gemini-3.1-flash-lite", (0.50, 1.50)),
    ];
    table.iter().find(|(k, _)| m.contains(k)).map(|(_, p)| *p)
}

/// The Gemini Batch API bills half the normal rate.
pub const BATCH_DISCOUNT: f64 = 0.5;

/// Published list prices (USD per audio minute) for common models, used for estimates.
pub fn known_price(model: &str) -> f64 {
    let m = model.to_lowercase();
    let table: &[(&str, f64)] = &[
        ("whisper-large-v3-turbo", 0.04 / 60.0),
        ("distil-whisper", 0.02 / 60.0),
        ("whisper-large-v3", 0.111 / 60.0),
        ("whisper-1", 0.006),
        ("gpt-4o-mini-transcribe", 0.003),
        ("gpt-4o-transcribe", 0.006),
        ("gemini-3.5-transcribe", 0.005),
        ("gemini", 0.002),
    ];
    table.iter().find(|(k, _)| m.contains(k)).map(|(_, p)| *p).unwrap_or(0.0)
}

impl ProviderConfig {
    pub fn effective_price(&self) -> f64 {
        self.price_per_minute.filter(|p| p.is_finite() && *p >= 0.0).unwrap_or_else(|| known_price(&self.model))
    }

    pub fn token_price(&self) -> Option<(f64, f64)> {
        let valid = |p: Option<f64>| p.filter(|v| v.is_finite() && *v >= 0.0);
        match (valid(self.price_input_per_mtok), valid(self.price_output_per_mtok)) {
            (Some(i), Some(o)) => Some((i, o)),
            _ => known_token_price(&self.model),
        }
    }

    /// Whether this provider/model returns speaker labels.
    pub fn diarizes(&self) -> bool {
        self.kind == ProviderKind::Gemini || self.model.contains("diarize")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SilenceMethod {
    /// Neural voice activity detection: drops anything that isn't speech (noise, music, hum).
    Speech,
    /// Volume threshold (ffmpeg silencedetect): only drops true quiet.
    Volume,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version for migrating saved settings; missing in files from v1.
    #[serde(default)]
    pub settings_version: u32,
    pub providers: Vec<ProviderConfig>,
    /// Used when speaker labels aren't needed (cheapest).
    pub default_provider: String,
    /// Used when speaker labels are requested.
    pub diarize_provider: String,
    pub language: String,
    /// Speed-up for providers without diarization (Whisper copes well up to ~2×).
    pub speed: f64,
    /// Speed-up for diarizing providers; kept lower since voice identification is untested at high speed.
    pub diarize_speed: f64,
    pub remove_silence: bool,
    pub silence_method: SilenceMethod,
    /// Speech probability above which a frame counts as speech (0..1). Lower keeps more.
    pub vad_threshold: f64,
    pub silence_threshold_db: f64,
    pub silence_min_duration: f64,
    /// Chunk length for Whisper-style providers (Groq caps uploads at 25 MB).
    pub chunk_minutes: f64,
    /// Chunk length for Gemini. Diarization requests are limited to 30 min of audio.
    pub gemini_chunk_minutes: f64,
    /// Model for "Overnight (batch)" jobs via the Gemini Batch API (~50% cheaper, results within 24 h).
    pub batch_model: String,
    /// Chunk length for batch jobs.
    pub batch_chunk_minutes: f64,
    /// Re-do batch parts that Google blocked or returned empty, right away with Gemini 3.5 Flash
    /// (3.5 Transcribe if that's blocked too).
    pub batch_retry_failed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            providers: vec![
                ProviderConfig {
                    id: "groq".into(),
                    name: "Groq".into(),
                    kind: ProviderKind::Openai,
                    base_url: "https://api.groq.com/openai/v1".into(),
                    api_key: String::new(),
                    model: "whisper-large-v3-turbo".into(),
                    price_per_minute: None,
                    price_input_per_mtok: None,
                    price_output_per_mtok: None,
                },
                ProviderConfig {
                    id: "openai".into(),
                    name: "OpenAI".into(),
                    kind: ProviderKind::Openai,
                    base_url: "https://api.openai.com/v1".into(),
                    api_key: String::new(),
                    model: "whisper-1".into(),
                    price_per_minute: None,
                    price_input_per_mtok: None,
                    price_output_per_mtok: None,
                },
                ProviderConfig {
                    id: "gemini".into(),
                    name: "Gemini".into(),
                    kind: ProviderKind::Gemini,
                    base_url: "https://generativelanguage.googleapis.com".into(),
                    api_key: String::new(),
                    model: "gemini-3.5-transcribe".into(),
                    price_per_minute: None,
                    price_input_per_mtok: None,
                    price_output_per_mtok: None,
                },
            ],
            settings_version: CURRENT_VERSION,
            default_provider: "groq".into(),
            diarize_provider: "gemini".into(),
            language: String::new(),
            speed: 1.5,
            diarize_speed: 1.25,
            remove_silence: true,
            silence_method: SilenceMethod::Speech,
            vad_threshold: 0.5,
            silence_threshold_db: -35.0,
            silence_min_duration: 0.7,
            chunk_minutes: 10.0,
            gemini_chunk_minutes: 10.0,
            batch_model: "gemini-3.8-flash".into(),
            batch_chunk_minutes: 10.0,
            batch_retry_failed: true,
        }
    }
}

const CURRENT_VERSION: u32 = 3;

pub fn load(path: &Path) -> Settings {
    let s: Settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    migrate(s)
}

/// v1 → v2: Groq becomes the default (Gemini only for speaker labels) and Whisper speed goes to 1.5×,
/// unless the user had changed those values.
fn migrate(mut s: Settings) -> Settings {
    if s.settings_version < 2 {
        if s.default_provider == "gemini" {
            s.default_provider = "groq".into();
        }
        if (s.speed - 1.25).abs() < 1e-9 {
            s.speed = 1.5;
        }
        s.settings_version = 2;
    }
    if s.settings_version < 3 {
        // v3: 25-minute Gemini chunks often skipped or looped on long meetings; 10 min is far more reliable.
        if (s.gemini_chunk_minutes - 25.0).abs() < 1e-9 {
            s.gemini_chunk_minutes = 10.0;
        }
        s.settings_version = 3;
    }
    if !s.providers.iter().any(|p| p.id == s.diarize_provider) {
        if let Some(p) = s.providers.iter().find(|p| p.diarizes()) {
            s.diarize_provider = p.id.clone();
        }
    }
    s
}

pub fn save(path: &Path, s: &Settings) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(s)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_lookup_prefers_specific_models() {
        assert!((known_price("whisper-large-v3-turbo") - 0.04 / 60.0).abs() < 1e-12);
        assert!((known_price("whisper-large-v3") - 0.111 / 60.0).abs() < 1e-12);
        assert_eq!(known_price("gpt-4o-transcribe-diarize"), 0.006);
        assert_eq!(known_price("gpt-4o-mini-transcribe"), 0.003);
        assert_eq!(known_price("my-local-model"), 0.0);
        assert_eq!(known_token_price("gemini-3.5-transcribe"), Some((2.0, 12.0)));
        assert_eq!(known_token_price("whisper-1"), None);
    }

    #[test]
    fn migrates_v1_settings() {
        let v1 = r#"{"providers": [], "default_provider": "gemini", "language": "", "speed": 1.25,
            "remove_silence": true, "silence_threshold_db": -35, "silence_min_duration": 0.7,
            "chunk_minutes": 10, "gemini_chunk_minutes": 25}"#;
        let s = migrate(serde_json::from_str(v1).unwrap());
        assert_eq!((s.default_provider.as_str(), s.speed, s.settings_version), ("groq", 1.5, 3));
        assert_eq!(s.gemini_chunk_minutes, 10.0);
        assert_eq!(s.silence_method, SilenceMethod::Speech);
        // A user's own choices are kept.
        let custom = v1.replace("\"gemini\"", "\"openai\"").replace("1.25", "1.1");
        let s = migrate(serde_json::from_str(&custom).unwrap());
        assert_eq!((s.default_provider.as_str(), s.speed), ("openai", 1.1));
        // Fresh defaults are already current.
        assert_eq!(migrate(Settings::default()).default_provider, "groq");
    }
}
