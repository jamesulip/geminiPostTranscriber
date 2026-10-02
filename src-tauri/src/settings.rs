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
}

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub providers: Vec<ProviderConfig>,
    pub default_provider: String,
    pub language: String,
    pub speed: f64,
    pub remove_silence: bool,
    pub silence_threshold_db: f64,
    pub silence_min_duration: f64,
    /// Chunk length for Whisper-style providers (Groq caps uploads at 25 MB).
    pub chunk_minutes: f64,
    /// Chunk length for Gemini. Diarization requests are limited to 30 min of audio.
    pub gemini_chunk_minutes: f64,
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
                },
                ProviderConfig {
                    id: "openai".into(),
                    name: "OpenAI".into(),
                    kind: ProviderKind::Openai,
                    base_url: "https://api.openai.com/v1".into(),
                    api_key: String::new(),
                    model: "whisper-1".into(),
                    price_per_minute: None,
                },
                ProviderConfig {
                    id: "gemini".into(),
                    name: "Gemini".into(),
                    kind: ProviderKind::Gemini,
                    base_url: "https://generativelanguage.googleapis.com".into(),
                    api_key: String::new(),
                    model: "gemini-3.5-transcribe".into(),
                    price_per_minute: None,
                },
            ],
            default_provider: "gemini".into(),
            language: String::new(),
            speed: 1.25,
            remove_silence: true,
            silence_threshold_db: -35.0,
            silence_min_duration: 0.7,
            chunk_minutes: 10.0,
            gemini_chunk_minutes: 25.0,
        }
    }
}

pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
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
    }
}
