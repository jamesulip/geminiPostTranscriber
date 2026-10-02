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
                },
                ProviderConfig {
                    id: "openai".into(),
                    name: "OpenAI".into(),
                    kind: ProviderKind::Openai,
                    base_url: "https://api.openai.com/v1".into(),
                    api_key: String::new(),
                    model: "whisper-1".into(),
                },
                ProviderConfig {
                    id: "gemini".into(),
                    name: "Gemini".into(),
                    kind: ProviderKind::Gemini,
                    base_url: "https://generativelanguage.googleapis.com".into(),
                    api_key: String::new(),
                    model: "gemini-3.5-transcribe".into(),
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
