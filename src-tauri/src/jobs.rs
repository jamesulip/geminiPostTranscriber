//! Background transcription jobs: preprocess → transcribe chunks → map times → store.

use crate::audio::{self, PreprocessOptions};
use crate::providers::{self, Request, Segment, TokenUsage};
use crate::settings::{ProviderConfig, ProviderKind};
use crate::{db, AppState};
use anyhow::{bail, Result};
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Deserialize)]
pub struct JobOptions {
    pub provider_id: String,
    pub model: Option<String>,
    pub language: Option<String>,
    pub speed: f64,
    pub remove_silence: bool,
    pub silence_threshold_db: f64,
    pub silence_min_duration: f64,
    pub chunk_minutes: f64,
    pub title: Option<String>,
    pub folder_id: Option<i64>,
    #[serde(default)]
    pub tag_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub id: String,
    pub stage: String,
    /// 0..1
    pub progress: f64,
    pub message: String,
    pub done: bool,
    pub error: Option<String>,
}

pub struct Job {
    pub id: String,
    /// Original file to copy into the library (None when re-transcribing).
    pub source: Option<PathBuf>,
    pub audio_path: PathBuf,
    pub provider: ProviderConfig,
    pub opts: JobOptions,
    pub cancel: Arc<AtomicBool>,
}

fn emit(app: &AppHandle, id: &str, stage: &str, progress: f64, message: impl Into<String>) {
    let _ = app.emit(
        "job-progress",
        Progress { id: id.into(), stage: stage.into(), progress, message: message.into(), done: false, error: None },
    );
}

/// What was actually sent to the provider, for usage/cost accounting.
#[derive(Default)]
struct Meter {
    /// First language reported by the provider.
    language: Option<String>,
    /// original / processed duration, to attribute original minutes to sent chunks.
    ratio: f64,
    sent_seconds: f64,
    requests: i64,
    tokens: TokenUsage,
}

impl Meter {
    fn add(&mut self, seconds: f64, tokens: TokenUsage, language: Option<String>) {
        if self.language.is_none() {
            self.language = language;
        }
        self.sent_seconds += seconds;
        self.requests += 1;
        self.tokens += tokens;
    }
}

pub async fn run(app: AppHandle, job: Job) {
    let state = app.state::<AppState>();
    let work = state.cache_dir.join("jobs").join(&job.id);
    let meter = Mutex::new(Meter::default());
    let result = execute(&app, &job, &work, &meter).await;
    let _ = std::fs::remove_dir_all(&work);
    state.cancels.lock().unwrap().remove(&job.id);

    let error = result.err().map(|e| format!("{e:#}"));
    {
        let db = state.db.lock().unwrap();
        let status = if error.is_some() { "error" } else { "done" };
        let _ = match &error {
            // Keeps the transcript usable when a re-transcription fails and a previous version exists.
            Some(e) => db::mark_failed(&db, &job.id, e),
            None => db::set_status(&db, &job.id, "done", None),
        };
        // Record whatever was billed, including partial work from failed/cancelled jobs.
        let m = meter.lock().unwrap();
        if m.requests > 0 {
            let title: String = db
                .query_row("SELECT title FROM transcripts WHERE id = ?1", [&job.id], |r| r.get(0))
                .unwrap_or_default();
            let _ = db::insert_usage(
                &db,
                &db::NewUsage {
                    transcript_id: &job.id,
                    title: &title,
                    provider: &job.provider.id,
                    provider_name: &job.provider.name,
                    model: &job.provider.model,
                    status,
                    original_seconds: m.sent_seconds * m.ratio,
                    sent_seconds: m.sent_seconds,
                    requests: m.requests,
                    input_tokens: m.tokens.input_tokens as i64,
                    output_tokens: m.tokens.output_tokens as i64,
                    price_per_minute: job.provider.effective_price(),
                },
            );
        }
    }
    let _ = app.emit(
        "job-progress",
        Progress {
            id: job.id.clone(),
            stage: if error.is_some() { "error".into() } else { "done".into() },
            progress: 1.0,
            message: error.clone().unwrap_or_else(|| "Done".into()),
            done: true,
            error,
        },
    );
}

async fn execute(app: &AppHandle, job: &Job, work: &std::path::Path, meter: &Mutex<Meter>) -> Result<()> {
    let state = app.state::<AppState>();
    let id = job.id.as_str();
    let cancelled = || job.cancel.load(Ordering::Relaxed);

    if let Some(src) = &job.source {
        emit(app, id, "copy", 0.02, "Copying audio into library");
        if let Some(dir) = job.audio_path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::copy(src, &job.audio_path).await?;
    }

    emit(app, id, "preprocess", 0.05, "Removing silence and preparing chunks");
    let o = &job.opts;
    let prepared = audio::preprocess(
        &job.audio_path,
        work,
        &PreprocessOptions {
            speed: o.speed,
            remove_silence: o.remove_silence,
            silence_threshold_db: o.silence_threshold_db,
            silence_min_duration: o.silence_min_duration,
            chunk_seconds: o.chunk_minutes * 60.0,
        },
    )
    .await?;
    {
        let db = state.db.lock().unwrap();
        db::set_duration(&db, id, prepared.original_duration)?;
        db::set_recording_meta(&db, id, prepared.recorded_at.as_deref(), None)?;
    }
    meter.lock().unwrap().ratio = prepared.original_duration / prepared.processed_duration.max(0.001);
    let n = prepared.chunks.len();
    let saved = 100.0 * (1.0 - prepared.processed_duration / prepared.original_duration.max(0.001));
    emit(
        app,
        id,
        "transcribe",
        0.15,
        format!("Transcribing {n} chunk(s) · {saved:.0}% shorter than original"),
    );
    if cancelled() {
        bail!("Cancelled");
    }

    let language = o.language.as_deref().filter(|l| !l.trim().is_empty());
    let client = &state.http;
    let cfg = &job.provider;
    let report = move |k: usize| {
        let p = 0.15 + 0.8 * (k as f64 / n as f64);
        emit(app, id, "transcribe", p, format!("Transcribed chunk {k}/{n}"));
    };

    let per_chunk: Vec<Vec<Segment>> = if providers::parallel(cfg) {
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let futs: Vec<_> = prepared
            .chunks
            .iter()
            .cloned()
            .map(|chunk| {
                let (client, cfg, cancel, done, app) =
                    (client.clone(), cfg.clone(), job.cancel.clone(), done.clone(), app.clone());
                let (language, id) = (language.map(str::to_string), id.to_string());
                async move {
                    if cancel.load(Ordering::Relaxed) {
                        bail!("Cancelled");
                    }
                    let req = Request {
                        path: &chunk.path,
                        duration: chunk.duration,
                        language: language.as_deref(),
                        known_speakers: &[],
                        previous_text: "",
                    };
                    let t = providers::transcribe(&client, &cfg, &req).await?;
                    meter.lock().unwrap().add(chunk.duration, t.usage, t.language);
                    let segs = t.segments;
                    let k = done.fetch_add(1, Ordering::Relaxed) + 1;
                    let p = 0.15 + 0.8 * (k as f64 / n as f64);
                    emit(&app, &id, "transcribe", p, format!("Transcribed chunk {k}/{n}"));
                    Ok::<_, anyhow::Error>(segs)
                }
            })
            .collect();
        futures::stream::iter(futs).buffered(3).try_collect().await?
    } else {
        // Sequential so each chunk can see the previous chunk's speakers and text.
        let mut all = Vec::with_capacity(n);
        let mut speakers: Vec<String> = Vec::new();
        let mut previous = String::new();
        for (k, chunk) in prepared.chunks.iter().enumerate() {
            if cancelled() {
                bail!("Cancelled");
            }
            let req = Request { path: &chunk.path, duration: chunk.duration, language, known_speakers: &speakers, previous_text: &previous };
            let t = providers::transcribe(client, cfg, &req).await?;
            meter.lock().unwrap().add(chunk.duration, t.usage, t.language);
            let segs = t.segments;
            for s in &segs {
                if let Some(sp) = &s.speaker {
                    if !speakers.contains(sp) {
                        speakers.push(sp.clone());
                    }
                }
            }
            previous = segs.iter().rev().take(4).rev().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
            all.push(segs);
            report(k + 1);
        }
        all
    };

    // Chunk-relative → processed → original time.
    emit(app, id, "merge", 0.97, "Merging");
    let merged = merge(&prepared.chunks, per_chunk, &prepared.map);
    if merged.is_empty() {
        bail!("The provider returned no speech");
    }
    let language = meter.lock().unwrap().language.clone();
    let mut db = state.db.lock().unwrap();
    db::commit_version(
        &mut db,
        id,
        &merged,
        &db::CompletedVersion {
            provider: &job.provider.id,
            model: &job.provider.model,
            language: o.language.as_deref().filter(|l| !l.trim().is_empty()),
            detected_language: language.as_deref(),
            processed_duration: prepared.processed_duration,
        },
    )?;
    Ok(())
}

/// Chunk-relative → processed → original time, sorted.
pub fn merge(chunks: &[audio::Chunk], per_chunk: Vec<Vec<Segment>>, map: &audio::TimeMap) -> Vec<Segment> {
    let mut merged: Vec<Segment> = chunks
        .iter()
        .zip(per_chunk)
        .flat_map(|(chunk, segs)| {
            segs.into_iter().map(move |s| Segment {
                start: map.to_original(chunk.offset + s.start),
                end: map.to_original_end(chunk.offset + s.end),
                ..s
            })
        })
        .collect();
    merged.sort_by(|a, b| a.start.total_cmp(&b.start));
    for s in &mut merged {
        s.end = s.end.max(s.start);
    }
    merged
}

pub fn chunk_minutes_default(kind: &ProviderKind, s: &crate::settings::Settings) -> f64 {
    match kind {
        ProviderKind::Gemini => s.gemini_chunk_minutes,
        ProviderKind::Openai => s.chunk_minutes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ProviderKind;

    /// Full path against a local mock server: `MOCK_URL=http://127.0.0.1:8765 MOCK_AUDIO=... cargo test -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn end_to_end_with_mock_provider() {
        let url = std::env::var("MOCK_URL").unwrap();
        let input = PathBuf::from(std::env::var("MOCK_AUDIO").unwrap());
        let work = std::env::temp_dir().join(format!("tx-e2e-{}", uuid::Uuid::new_v4()));
        let opts = PreprocessOptions { speed: 1.25, remove_silence: true, silence_threshold_db: -35.0, silence_min_duration: 0.7, chunk_seconds: 30.0 };
        let prepared = audio::preprocess(&input, &work, &opts).await.unwrap();
        let cfg = ProviderConfig { id: "mock".into(), name: "Mock".into(), kind: ProviderKind::Openai, base_url: url, api_key: "test".into(), model: "whisper-mock".into(), price_per_minute: None };
        let client = reqwest::Client::new();
        let mut per_chunk = vec![];
        for c in &prepared.chunks {
            let req = Request { path: &c.path, duration: c.duration, language: None, known_speakers: &[], previous_text: "" };
            per_chunk.push(providers::transcribe(&client, &cfg, &req).await.unwrap().segments);
        }
        let merged = merge(&prepared.chunks, per_chunk, &prepared.map);
        println!("orig {:.2}s processed {:.2}s chunks {}", prepared.original_duration, prepared.processed_duration, prepared.chunks.len());
        for s in &merged { println!("{:7.2} {:7.2} {}", s.start, s.end, s.text); }
        let _ = std::fs::remove_dir_all(work);
    }
}
