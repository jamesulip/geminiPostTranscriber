//! Background transcription jobs: preprocess → transcribe chunks → map times → store.

use crate::audio::{self, PreprocessOptions};
use crate::providers::{self, Request, Segment};
use crate::settings::{ProviderConfig, ProviderKind};
use crate::{db, AppState};
use anyhow::{bail, Result};
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

pub async fn run(app: AppHandle, job: Job) {
    let state = app.state::<AppState>();
    let work = state.cache_dir.join("jobs").join(&job.id);
    let result = execute(&app, &job, &work).await;
    let _ = std::fs::remove_dir_all(&work);
    state.cancels.lock().unwrap().remove(&job.id);

    let error = result.err().map(|e| format!("{e:#}"));
    {
        let db = state.db.lock().unwrap();
        let status = if error.is_some() { "error" } else { "done" };
        let _ = db::set_status(&db, &job.id, status, error.as_deref());
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

async fn execute(app: &AppHandle, job: &Job, work: &std::path::Path) -> Result<()> {
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
        db::set_durations(&db, id, prepared.original_duration, prepared.processed_duration)?;
    }
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
    let done = std::sync::atomic::AtomicUsize::new(0);
    let report = |k: usize| {
        let p = 0.15 + 0.8 * (k as f64 / n as f64);
        emit(app, id, "transcribe", p, format!("Transcribed chunk {k}/{n}"));
    };

    let per_chunk: Vec<Vec<Segment>> = if providers::parallel(cfg) {
        futures::stream::iter(prepared.chunks.iter())
            .map(|chunk| {
                let done = &done;
                async move {
                    if cancelled() {
                        bail!("Cancelled");
                    }
                    let req = Request {
                        path: &chunk.path,
                        duration: chunk.duration,
                        language,
                        known_speakers: &[],
                        previous_text: "",
                    };
                    let segs = providers::transcribe(client, cfg, &req).await?;
                    report(done.fetch_add(1, Ordering::Relaxed) + 1);
                    Ok(segs)
                }
            })
            .buffered(3)
            .try_collect()
            .await?
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
            let segs = providers::transcribe(client, cfg, &req).await?;
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
    let map = &prepared.map;
    let mut merged: Vec<Segment> = prepared
        .chunks
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
    if merged.is_empty() {
        bail!("The provider returned no speech");
    }
    let mut db = state.db.lock().unwrap();
    db::replace_segments(&mut db, id, &merged)?;
    Ok(())
}

pub fn chunk_minutes_default(kind: &ProviderKind, s: &crate::settings::Settings) -> f64 {
    match kind {
        ProviderKind::Gemini => s.gemini_chunk_minutes,
        ProviderKind::Openai => s.chunk_minutes,
    }
}
