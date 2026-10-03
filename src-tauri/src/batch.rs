//! "Overnight" transcription through the Gemini Batch API: submit all chunks as one batch
//! (~50% cheaper), then poll until Google finishes (usually hours, at most ~24 h) and save the
//! result. Pending batches survive app restarts: everything needed is stored on the transcript.

use crate::audio::{self, TimeMap};
use crate::jobs::{emit, merge, Job, Progress};
use crate::providers::gemini::transcription_request;
use crate::providers::gemini_batch::{self, State};
use crate::providers::{Segment, TokenUsage};
use crate::settings::{self, ProviderConfig};
use crate::{db, quality, AppState};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// Pending batches older than this are given up on (Google expires them after 48 h).
const GIVE_UP_HOURS: i64 = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchChunk {
    pub offset: f64,
    pub duration: f64,
}

/// Everything needed to finish a batch later, stored as JSON on the transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMeta {
    pub name: String,
    pub submitted_at: String,
    pub provider_id: String,
    pub model: String,
    pub language: Option<String>,
    pub condensed: bool,
    pub original_duration: f64,
    pub processed_duration: f64,
    pub chunks: Vec<BatchChunk>,
    pub map: TimeMap,
}

fn key(i: usize) -> String {
    format!("chunk-{i}")
}

/// Build the batch input from the prepared chunks, submit it and mark the transcript "queued".
pub async fn submit(app: &AppHandle, job: &Job, prepared: &audio::Prepared) -> Result<()> {
    let state = app.state::<AppState>();
    let language = job.opts.language.as_deref().filter(|l| !l.trim().is_empty());
    let mut jsonl = String::new();
    for (i, chunk) in prepared.chunks.iter().enumerate() {
        let bytes = tokio::fs::read(&chunk.path).await?;
        // Chunks are independent in a batch, so no "previous speakers" context.
        jsonl.push_str(&gemini_batch::jsonl_line(&key(i), &transcription_request(&bytes, language, &[], "")));
        jsonl.push('\n');
    }
    let title: String = {
        let db = state.db.lock().unwrap();
        db.query_row("SELECT title FROM transcripts WHERE id = ?1", [&job.id], |r| r.get(0)).unwrap_or_default()
    };
    let name = gemini_batch::create(&state.http, &job.provider, jsonl.into_bytes(), &format!("transcriber: {title}")).await?;

    let meta = BatchMeta {
        name,
        submitted_at: chrono::Local::now().to_rfc3339(),
        provider_id: job.provider.id.clone(),
        model: job.provider.model.clone(),
        language: language.map(str::to_string),
        condensed: job.opts.remove_silence,
        original_duration: prepared.original_duration,
        processed_duration: prepared.processed_duration,
        chunks: prepared.chunks.iter().map(|c| BatchChunk { offset: c.offset, duration: c.duration }).collect(),
        map: prepared.map.clone(),
    };
    let db = state.db.lock().unwrap();
    db::set_batch(&db, &job.id, Some(&serde_json::to_string(&meta)?))?;
    db::set_status(&db, &job.id, "queued", None)?;
    Ok(())
}

fn provider_for(app: &AppHandle, meta: &BatchMeta) -> Result<ProviderConfig> {
    let state = app.state::<AppState>();
    let s = settings::load(&state.data_dir.join("settings.json"));
    let mut p = s
        .providers
        .into_iter()
        .find(|p| p.id == meta.provider_id)
        .ok_or_else(|| anyhow!("Provider {} no longer exists", meta.provider_id))?;
    p.model = meta.model.clone();
    Ok(p)
}

fn finish(app: &AppHandle, id: &str, error: Option<String>) {
    let state = app.state::<AppState>();
    {
        let db = state.db.lock().unwrap();
        let _ = db::set_batch(&db, id, None);
        let _ = match &error {
            Some(e) => db::mark_failed(&db, id, e),
            None => db::set_status(&db, id, "done", None),
        };
    }
    state.progress.lock().unwrap().remove(id);
    let _ = app.emit(
        "job-progress",
        Progress {
            id: id.to_string(),
            stage: if error.is_some() { "error".into() } else { "done".into() },
            progress: 1.0,
            message: error.clone().unwrap_or_else(|| "Done".into()),
            done: true,
            error,
        },
    );
}

/// Check every pending batch once; save finished ones.
pub async fn poll(app: &AppHandle) {
    let queued = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        db::queued_batches(&db).unwrap_or_default()
    };
    for (id, json) in queued {
        if let Err(e) = poll_one(app, &id, &json).await {
            // Network hiccups etc.: leave it queued and try again next round.
            emit(app, &id, "batch", 0.8, format!("Waiting at Google — couldn't check just now ({e:#}); will retry"));
        }
    }
}

async fn poll_one(app: &AppHandle, id: &str, json: &str) -> Result<()> {
    let meta: BatchMeta = serde_json::from_str(json)?;
    let cfg = provider_for(app, &meta)?;
    let state = app.state::<AppState>();
    let status = gemini_batch::status(&state.http, &cfg, &meta.name).await?;
    let submitted = chrono::DateTime::parse_from_rfc3339(&meta.submitted_at).ok();
    let age_h = submitted.map(|t| (chrono::Local::now().fixed_offset() - t).num_hours()).unwrap_or(0);
    match status.state {
        State::Pending | State::Running if age_h >= GIVE_UP_HOURS => {
            finish(app, id, Some("The overnight batch didn't finish within 48 hours (expired at Google). Retry to send it again.".into()));
        }
        State::Pending | State::Running => {
            let what = if status.state == State::Running { "processing" } else { "queued" };
            let since = submitted.map(|t| t.format("%b %-d, %H:%M").to_string()).unwrap_or_default();
            emit(app, id, "batch", 0.8, format!("Waiting at Google ({what}) — sent {since}, results usually within 24 h"));
        }
        State::Failed(e) => finish(app, id, Some(format!("The overnight batch failed at Google: {e}"))),
        State::Expired => finish(app, id, Some("The overnight batch expired at Google (not done within 48 h). Retry to send it again.".into())),
        State::Cancelled => finish(app, id, Some("Cancelled".into())),
        State::Succeeded => {
            emit(app, id, "merge", 0.95, "Downloading results from Google");
            let results = match (&status.results_file, &status.inline) {
                (Some(file), _) => gemini_batch::parse_results_file(&gemini_batch::download(&state.http, &cfg, file).await?),
                (None, Some(lines)) => gemini_batch::parse_results(lines.iter().cloned()),
                (None, None) => return Err(anyhow!("Google reported the batch as done but returned no results")),
            };
            save_results(app, id, &meta, &cfg, results)?;
        }
    }
    Ok(())
}

/// Turn per-chunk results into a transcript version, with quality notes and a usage record.
fn save_results(
    app: &AppHandle,
    id: &str,
    meta: &BatchMeta,
    cfg: &ProviderConfig,
    mut results: std::collections::HashMap<String, Result<crate::providers::Transcription, String>>,
) -> Result<()> {
    let state = app.state::<AppState>();
    let checks = quality::CheckOptions { condensed: meta.condensed, want_speakers: true };
    let chunks: Vec<audio::Chunk> = meta
        .chunks
        .iter()
        .map(|c| audio::Chunk { path: Default::default(), offset: c.offset, duration: c.duration })
        .collect();
    let (mut per_chunk, mut notes, mut usage, mut ok_secs) = (Vec::new(), Vec::new(), TokenUsage::default(), 0.0);
    for (i, c) in chunks.iter().enumerate() {
        let span = format!("{}–{} min of sent audio", (c.offset / 60.0) as u64, ((c.offset + c.duration) / 60.0).ceil() as u64);
        match results.remove(&key(i)) {
            Some(Ok(t)) => {
                usage += t.usage;
                ok_secs += c.duration;
                let segs: Vec<Segment> = t
                    .segments
                    .into_iter()
                    .map(|mut s| {
                        s.start = s.start.clamp(0.0, c.duration);
                        s.end = s.end.clamp(s.start, c.duration);
                        s
                    })
                    .collect();
                if let Some(problem) = quality::check(&segs, c.duration, &checks) {
                    notes.push(format!("Part {} ({span}): {problem}", i + 1));
                }
                per_chunk.push(quality::remove_loops(segs).0);
            }
            Some(Err(e)) => {
                notes.push(format!("Part {} ({span}): Google returned an error — {e}", i + 1));
                per_chunk.push(Vec::new());
            }
            None => {
                notes.push(format!("Part {} ({span}): missing from Google's results", i + 1));
                per_chunk.push(Vec::new());
            }
        }
    }
    let merged = merge(&chunks, per_chunk, &meta.map);
    if merged.is_empty() {
        finish(app, id, Some(format!("The overnight batch returned no text. {}", notes.join("; "))));
        return Ok(());
    }
    if !notes.is_empty() {
        notes.push("Use “Fix problems” to re-do these parts now.".into());
    }

    let mut db = state.db.lock().unwrap();
    db::commit_version(
        &mut db,
        id,
        &merged,
        &db::CompletedVersion {
            provider: &cfg.id,
            model: &cfg.model,
            language: meta.language.as_deref(),
            detected_language: None,
            notes: (!notes.is_empty()).then(|| notes.join("\n")),
            processed_duration: meta.processed_duration,
        },
    )?;
    // Usage at batch prices (half the normal rate).
    let original = meta.original_duration * ok_secs / meta.processed_duration.max(0.001);
    let per_minute = cfg.effective_price() * settings::BATCH_DISCOUNT;
    let (input, output) = (usage.input_tokens as i64, usage.output_tokens as i64);
    let (cost_usd, saved_usd) = match cfg.token_price() {
        Some((pi, po)) if input > 0 => {
            let (cost, saved) = db::token_cost(input, output, (pi, po), original, ok_secs);
            (cost * settings::BATCH_DISCOUNT, saved * settings::BATCH_DISCOUNT)
        }
        _ => db::minute_cost(original, ok_secs, per_minute),
    };
    let title: String = db.query_row("SELECT title FROM transcripts WHERE id = ?1", [id], |r| r.get(0)).unwrap_or_default();
    db::insert_usage(
        &db,
        &db::NewUsage {
            transcript_id: id,
            title: &title,
            provider: &cfg.id,
            provider_name: &format!("{} (batch)", cfg.name),
            model: &cfg.model,
            status: "done",
            original_seconds: original,
            sent_seconds: ok_secs,
            requests: chunks.len() as i64,
            input_tokens: input,
            output_tokens: output,
            price_per_minute: per_minute,
            cost_usd,
            saved_usd,
        },
    )?;
    drop(db);
    finish(app, id, None);
    Ok(())
}

/// Cancel a pending batch at Google and mark the transcript as cancelled.
pub async fn cancel(app: &AppHandle, id: &str) -> Result<()> {
    let json = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        db.query_row("SELECT batch FROM transcripts WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0))?
    };
    if let Some(json) = json {
        let meta: BatchMeta = serde_json::from_str(&json)?;
        let cfg = provider_for(app, &meta)?;
        // Already finished/cancelled at Google is fine.
        let _ = gemini_batch::cancel(&app.state::<AppState>().http, &cfg, &meta.name).await;
    }
    finish(app, id, Some("Cancelled".into()));
    Ok(())
}

/// Poll forever: shortly after startup, then every few minutes.
pub async fn poll_loop(app: AppHandle) {
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    loop {
        poll(&app).await;
        tokio::time::sleep(std::time::Duration::from_secs(180)).await;
    }
}
