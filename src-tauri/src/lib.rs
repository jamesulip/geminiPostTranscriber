pub mod ai;
mod audio;
mod batch;
pub mod db;
mod diarize;
mod export;
mod jobs;
pub mod providers;
mod quality;
mod repair;
pub mod settings;

use rusqlite::Connection;
use settings::Settings;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};

pub struct AppState {
    pub db: Mutex<Connection>,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub http: reqwest::Client,
    pub cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Latest progress of running jobs (for windows that reload mid-job).
    pub progress: Mutex<HashMap<String, jobs::Progress>>,
    /// Cancel flags of running AI work, by key (`digest:<id>`, `ask:<thread>`, `embed:library`).
    pub ai_jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl AppState {
    fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// Settings --------------------------------------------------------------------

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    settings::load(&state.settings_path())
}

#[tauri::command]
fn save_settings(state: State<AppState>, settings: Settings) -> CmdResult<()> {
    settings::save(&state.settings_path(), &settings).map_err(err)
}

#[tauri::command]
fn default_chunk_minutes(state: State<AppState>, provider_id: String) -> f64 {
    let s = settings::load(&state.settings_path());
    s.providers
        .iter()
        .find(|p| p.id == provider_id)
        .map(|p| jobs::chunk_minutes_default(&p.kind, &s))
        .unwrap_or(s.chunk_minutes)
}

// Jobs ------------------------------------------------------------------------

fn spawn_job(
    app: &AppHandle,
    state: &AppState,
    id: String,
    source: Option<PathBuf>,
    audio_path: PathBuf,
    opts: jobs::JobOptions,
) -> CmdResult<()> {
    spawn_job_with(app, state, id, source, audio_path, opts, None)
}

pub(crate) fn spawn_job_with(
    app: &AppHandle,
    state: &AppState,
    id: String,
    source: Option<PathBuf>,
    audio_path: PathBuf,
    mut opts: jobs::JobOptions,
    repair: Option<Vec<audio::Interval>>,
) -> CmdResult<()> {
    let s = settings::load(&state.settings_path());
    let mut provider = s
        .providers
        .iter()
        .find(|p| p.id == opts.provider_id)
        .cloned()
        .ok_or("Unknown provider")?;
    if let Some(m) = opts.model.as_ref().filter(|m| !m.trim().is_empty()) {
        provider.model = m.trim().to_string();
    }
    opts.model = Some(provider.model.clone());
    if opts.batch && (provider.kind != settings::ProviderKind::Gemini || provider.model.contains("transcribe")) {
        return Err("Overnight batch needs a general Gemini model such as gemini-3.8-flash (Gemini 3.5 Transcribe has no batch mode).".into());
    }
    // Repairs reuse the last full run's options; only full runs define them (and its chunk cache).
    if repair.is_none() {
        if let Ok(json) = serde_json::to_string(&opts) {
            let _ = db::set_last_options(&state.db.lock().unwrap(), &id, &json);
        }
    }
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancels.lock().unwrap().insert(id.clone(), cancel.clone());
    let job = jobs::Job { id, source, audio_path, provider, opts, cancel, repair };
    tauri::async_runtime::spawn(jobs::run(app.clone(), job));
    Ok(())
}

#[tauri::command]
fn start_transcription(app: AppHandle, state: State<AppState>, path: String, options: jobs::JobOptions) -> CmdResult<String> {
    let src = PathBuf::from(&path);
    if !src.is_file() {
        return Err(format!("File not found: {path}"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let source_name = src.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let ext = src.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "audio".into());
    let audio_path = state.data_dir.join("audio").join(format!("{id}.{ext}"));
    let title = options
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| src.file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
    // Fallback recording date: the file's modification time (refined later from metadata).
    let recorded_at = std::fs::metadata(&src)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339());
    let s = settings::load(&state.settings_path());
    let model = options
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| s.providers.iter().find(|p| p.id == options.provider_id).map(|p| p.model.clone()))
        .unwrap_or_default();
    {
        let db = state.db.lock().unwrap();
        db::insert_transcript(
            &db,
            &db::NewTranscript {
                id: &id,
                title: &title,
                folder_id: options.folder_id,
                audio_path: &audio_path.to_string_lossy(),
                source_name: &source_name,
                provider: &options.provider_id,
                model: &model,
                language: options.language.as_deref().filter(|l| !l.is_empty()),
                recorded_at: recorded_at.as_deref(),
                tag_ids: &options.tag_ids,
            },
        )
        .map_err(err)?;
    }
    spawn_job(&app, &state, id.clone(), Some(src), audio_path, options)?;
    Ok(id)
}

/// Re-run transcription on a library item (e.g. after fixing an API key or changing provider).
#[tauri::command]
fn retranscribe(app: AppHandle, state: State<AppState>, id: String, options: jobs::JobOptions) -> CmdResult<()> {
    let audio_path = {
        let db = state.db.lock().unwrap();
        let t = db::get_transcript(&db, &id).map_err(err)?.ok_or("Transcript not found")?;
        // Provider/model/language are only updated once the new version succeeds.
        db::set_status(&db, &id, "processing", None).map_err(err)?;
        PathBuf::from(t.audio_path)
    };
    spawn_job(&app, &state, id, None, audio_path, options)
}

/// Seconds of speech (VAD on the original audio) inside each `[start, end]` stretch, so the UI
/// can tell real pauses from audio the model skipped.
#[tauri::command]
async fn speech_in_gaps(state: State<'_, AppState>, id: String, gaps: Vec<(f64, f64)>) -> CmdResult<Vec<f64>> {
    let audio_path = {
        let db = state.db.lock().unwrap();
        PathBuf::from(db::get_transcript(&db, &id).map_err(err)?.ok_or("Transcript not found")?.audio_path)
    };
    std::fs::create_dir_all(&state.cache_dir).map_err(err)?;
    let scratch = state.cache_dir.join(format!("gaps-{}.wav", uuid::Uuid::new_v4()));
    let mut out = Vec::with_capacity(gaps.len());
    for (a, b) in gaps {
        out.push(audio::speech_seconds(&audio_path, a, (b - a).max(0.0), &scratch).await.map_err(err)?);
    }
    Ok(out)
}

/// Path of an MP3 copy of the transcript's audio, made on first use, for recordings the webview
/// can't play directly.
#[tauri::command]
async fn playback_audio(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let original = {
        let db = state.db.lock().unwrap();
        PathBuf::from(db::get_transcript(&db, &id).map_err(err)?.ok_or("Transcript not found")?.audio_path)
    };
    let p = audio::ensure_playback_copy(&original).await.map_err(err)?;
    Ok(p.to_string_lossy().to_string())
}

#[derive(serde::Serialize)]
pub struct AudioLevels {
    /// Seconds per value.
    pub step: f64,
    /// 0..255, from -60 dBFS to 0 dBFS.
    pub levels: Vec<u8>,
}

/// Loudness over time of the transcript's audio (for the waveform lane and the level meter).
#[tauri::command]
async fn audio_levels(state: State<'_, AppState>, id: String) -> CmdResult<AudioLevels> {
    let original = {
        let db = state.db.lock().unwrap();
        PathBuf::from(db::get_transcript(&db, &id).map_err(err)?.ok_or("Transcript not found")?.audio_path)
    };
    let levels = audio::ensure_levels(&original).await.map_err(err)?;
    Ok(AudioLevels { step: audio::LEVEL_STEP, levels })
}

/// Find who spoke when with the local diarizer and relabel the current version's segments.
/// Runs as a job (progress under the transcript id); the previous labeling is kept as a version.
#[tauri::command]
fn diarize_transcript(app: AppHandle, state: State<AppState>, id: String, num_speakers: Option<i32>) -> CmdResult<()> {
    if num_speakers.is_some_and(|n| !(1..=20).contains(&n)) {
        return Err("Number of speakers must be between 1 and 20".into());
    }
    {
        let mut cancels = state.cancels.lock().unwrap();
        if cancels.contains_key(&id) {
            return Err("A job is already running for this transcript".into());
        }
        cancels.insert(id.clone(), Arc::new(AtomicBool::new(false)));
    }
    let cancel = state.cancels.lock().unwrap().get(&id).cloned().unwrap();
    tauri::async_runtime::spawn(async move {
        let result = diarize::run(app.clone(), id.clone(), num_speakers, cancel).await;
        let state = app.state::<AppState>();
        state.cancels.lock().unwrap().remove(&id);
        state.progress.lock().unwrap().remove(&id);
        let (message, error) = match result {
            Ok(a) => (format!("Found {} speaker(s) in {:.0} min", a.speakers.len(), a.seconds / 60.0), None),
            Err(e) => (String::new(), Some(format!("{e:#}"))),
        };
        use tauri::Emitter;
        let _ = app.emit(
            "job-progress",
            jobs::Progress { id, stage: diarize::STAGE.into(), progress: 1.0, message, done: true, error },
        );
    });
    Ok(())
}

#[derive(serde::Serialize)]
pub struct DiarizationInfo {
    /// Models already downloaded (the first run fetches about 45 MB).
    pub models_ready: bool,
    /// Audio seconds processed per second on this machine (last run, else a default).
    pub realtime_factor: f64,
}

#[tauri::command]
fn diarization_info(state: State<AppState>) -> DiarizationInfo {
    DiarizationInfo { models_ready: diarize::models_ready(&state.data_dir), realtime_factor: diarize::load_rate(&state.data_dir) }
}

/// Stretches of the current version that look wrong (missing text, loops, impossible timings,
/// missing speaker labels), for re-transcribing only those parts.
#[tauri::command]
async fn find_problems(state: State<'_, AppState>, id: String) -> CmdResult<Vec<repair::ProblemRange>> {
    let (t, want_speakers) = {
        let db = state.db.lock().unwrap();
        let t = db::get_transcript(&db, &id).map_err(err)?.ok_or("Transcript not found")?;
        let s = settings::load(&state.settings_path());
        let diarized = s.providers.iter().find(|p| p.id == t.summary.provider).is_some_and(|p| p.diarizes())
            || t.segments.iter().any(|s| s.speaker.is_some());
        (t, diarized)
    };
    let mut ranges = repair::text_problems(&t.segments, want_speakers);
    let scratch = state.cache_dir.join(format!("vad-{id}.wav"));
    std::fs::create_dir_all(&state.cache_dir).map_err(err)?;
    ranges.extend(
        repair::missing_speech(&t.segments, t.summary.duration, std::path::Path::new(&t.audio_path), &scratch)
            .await
            .map_err(err)?,
    );
    Ok(repair::merge_ranges(ranges, t.summary.duration))
}

/// Re-transcribe only the given stretches (seconds of original audio) and splice them in.
#[tauri::command]
fn repair_transcript(
    app: AppHandle,
    state: State<AppState>,
    id: String,
    ranges: Vec<(f64, f64)>,
    options: jobs::JobOptions,
) -> CmdResult<()> {
    if ranges.is_empty() {
        return Err("Choose at least one part to fix".into());
    }
    let audio_path = {
        let db = state.db.lock().unwrap();
        let t = db::get_transcript(&db, &id).map_err(err)?.ok_or("Transcript not found")?;
        db::set_status(&db, &id, "processing", None).map_err(err)?;
        PathBuf::from(t.audio_path)
    };
    let mut ranges: Vec<audio::Interval> =
        ranges.into_iter().filter(|(a, b)| b > a).map(|(start, end)| audio::Interval { start, end }).collect();
    ranges.sort_by(|a, b| a.start.total_cmp(&b.start));
    spawn_job_with(&app, &state, id, None, audio_path, options, Some(ranges))
}

/// Latest progress of every running job.
#[tauri::command]
fn job_progress(state: State<AppState>) -> Vec<jobs::Progress> {
    state.progress.lock().unwrap().values().cloned().collect()
}

#[tauri::command]
fn cancel_job(app: AppHandle, state: State<AppState>, id: String) {
    if let Some(c) = state.cancels.lock().unwrap().get(&id) {
        c.store(true, Ordering::Relaxed);
        return;
    }
    // Not running locally: maybe an overnight batch waiting at Google.
    let queued = db::get_transcript(&state.db.lock().unwrap(), &id)
        .ok()
        .flatten()
        .is_some_and(|t| t.summary.status == "queued");
    if queued {
        tauri::async_runtime::spawn(async move {
            if let Err(e) = batch::cancel(&app, &id).await {
                eprintln!("cancel batch {id}: {e:#}");
            }
        });
    }
}

#[derive(serde::Serialize)]
struct BatchList {
    batches: Vec<batch::BatchView>,
    warning: Option<String>,
}

/// All overnight batches (Google's list merged with local records) for the Batches page.
#[tauri::command]
async fn list_batches(app: AppHandle) -> CmdResult<BatchList> {
    let (batches, warning) = batch::list(&app).await.map_err(err)?;
    Ok(BatchList { batches, warning })
}

/// Check overnight batches at Google right away (instead of waiting for the next round).
#[tauri::command]
async fn check_batches(app: AppHandle) -> CmdResult<()> {
    batch::poll(&app).await;
    Ok(())
}

// Library -----------------------------------------------------------------------

#[tauri::command]
fn list_transcripts(state: State<AppState>, filter: db::ListFilter) -> CmdResult<Vec<db::TranscriptSummary>> {
    db::list_transcripts(&state.db.lock().unwrap(), &filter).map_err(err)
}

#[tauri::command]
fn get_transcript(state: State<AppState>, id: String) -> CmdResult<Option<db::TranscriptDetail>> {
    db::get_transcript(&state.db.lock().unwrap(), &id).map_err(err)
}

#[tauri::command]
fn update_transcript(
    state: State<AppState>,
    id: String,
    title: String,
    folder_id: Option<i64>,
    tag_ids: Vec<i64>,
) -> CmdResult<()> {
    db::update_transcript(&state.db.lock().unwrap(), &id, &title, folder_id, &tag_ids).map_err(err)
}

/// Stop a running job or pending overnight batch so Google doesn't keep working (and billing) on it.
fn stop_work(app: &AppHandle, state: &AppState, id: &str) {
    let queued = db::get_transcript(&state.db.lock().unwrap(), id).ok().flatten().is_some_and(|t| t.summary.status == "queued");
    if queued {
        let (app, id) = (app.clone(), id.to_string());
        tauri::async_runtime::spawn(async move {
            let _ = batch::cancel(&app, &id).await;
        });
    }
    if let Some(c) = state.cancels.lock().unwrap().get(id) {
        c.store(true, Ordering::Relaxed);
    }
}

/// Permanently delete a transcript: database row plus its audio copy and caches.
fn purge_transcript(app: &AppHandle, state: &AppState, id: &str) -> CmdResult<()> {
    stop_work(app, state, id);
    let audio = db::delete_transcript(&state.db.lock().unwrap(), id).map_err(err)?;
    let _ = std::fs::remove_dir_all(state.cache_dir.join("results").join(id));
    let _ = std::fs::remove_dir_all(state.data_dir.join("raw").join(id));
    if let Some(p) = audio {
        let _ = std::fs::remove_file(audio::playback_path(std::path::Path::new(&p)));
        let _ = std::fs::remove_file(std::path::Path::new(&p).with_extension("levels.bin"));
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

#[tauri::command]
fn delete_transcript(app: AppHandle, state: State<AppState>, id: String) -> CmdResult<()> {
    purge_transcript(&app, &state, &id)
}

#[tauri::command]
fn trash_transcript(app: AppHandle, state: State<AppState>, id: String) -> CmdResult<()> {
    stop_work(&app, &state, &id);
    db::trash_transcript(&state.db.lock().unwrap(), &id).map_err(err)
}

#[tauri::command]
fn restore_transcript(state: State<AppState>, id: String) -> CmdResult<()> {
    db::restore_transcript(&state.db.lock().unwrap(), &id).map_err(err)
}

#[tauri::command]
fn empty_trash(app: AppHandle, state: State<AppState>) -> CmdResult<()> {
    let ids = db::trashed_ids(&state.db.lock().unwrap()).map_err(err)?;
    for id in ids {
        purge_transcript(&app, &state, &id)?;
    }
    Ok(())
}

#[tauri::command]
fn set_flagged(state: State<AppState>, id: String, flagged: bool) -> CmdResult<()> {
    db::set_flagged(&state.db.lock().unwrap(), &id, flagged).map_err(err)
}

#[tauri::command]
fn rename_speaker(state: State<AppState>, id: String, label: String, name: String) -> CmdResult<()> {
    db::rename_speaker(&state.db.lock().unwrap(), &id, &label, &name).map_err(err)
}

#[tauri::command]
fn update_segment(state: State<AppState>, segment_id: i64, text: String, speaker: Option<String>) -> CmdResult<()> {
    db::update_segment(&state.db.lock().unwrap(), segment_id, &text, speaker.as_deref()).map_err(err)
}

#[tauri::command]
fn search(state: State<AppState>, query: String) -> CmdResult<Vec<db::SearchHit>> {
    db::search(&state.db.lock().unwrap(), &query, 50).map_err(err)
}

/// Export the current version, or an archived one when `version_id` is given.
#[tauri::command]
fn export_transcript(
    state: State<AppState>,
    id: String,
    format: String,
    path: String,
    version_id: Option<i64>,
) -> CmdResult<()> {
    let db = state.db.lock().unwrap();
    let mut t = db::get_transcript(&db, &id).map_err(err)?.ok_or("Transcript not found")?;
    if let Some(vid) = version_id {
        let v = db::get_version(&db, vid).map_err(err)?.ok_or("Version not found")?;
        t.segments = v.segments;
        t.speakers = v.speakers;
        t.summary.model = v.info.model;
        t.summary.provider = v.info.provider;
        t.summary.language = v.info.language;
        t.summary.version = v.info.number;
    }
    std::fs::write(path, export::render(&t, &format)).map_err(err)
}

#[tauri::command]
fn list_versions(state: State<AppState>, id: String) -> CmdResult<Vec<db::VersionInfo>> {
    db::list_versions(&state.db.lock().unwrap(), &id).map_err(err)
}

#[tauri::command]
fn get_version(state: State<AppState>, version_id: i64) -> CmdResult<Option<db::VersionDetail>> {
    db::get_version(&state.db.lock().unwrap(), version_id).map_err(err)
}

#[tauri::command]
fn restore_version(state: State<AppState>, version_id: i64) -> CmdResult<()> {
    db::restore_version(&mut state.db.lock().unwrap(), version_id).map_err(err)
}

#[tauri::command]
fn delete_version(state: State<AppState>, version_id: i64) -> CmdResult<()> {
    db::delete_version(&state.db.lock().unwrap(), version_id).map_err(err)
}

#[tauri::command]
fn usage_summary(state: State<AppState>) -> CmdResult<db::UsageSummary> {
    db::usage_summary(&state.db.lock().unwrap()).map_err(err)
}

#[derive(serde::Serialize)]
struct KnownPrice {
    per_minute: f64,
    /// USD per 1M input/output tokens, for token-billed models.
    tokens: Option<(f64, f64)>,
    /// Text rates for the AI assistant.
    text: Option<settings::TextPrice>,
}

/// Built-in price estimates for a model, shown as Settings placeholders.
#[tauri::command]
fn known_price(model: String) -> KnownPrice {
    KnownPrice {
        per_minute: settings::known_price(&model),
        tokens: settings::known_token_price(&model),
        text: settings::known_text_price(&model),
    }
}

// AI assistant ------------------------------------------------------------------------

/// Run `work` with an `AiCtx` on the app's state, forwarding events to the window as `ai-event`.
/// The cancel flag is registered under `key` for `ai_cancel`. Returns Err if that key is already running.
fn ai_run<F, T>(app: &AppHandle, key: String, work: F) -> CmdResult<tauri::async_runtime::JoinHandle<Result<T, String>>>
where
    F: for<'a> FnOnce(&'a ai::AiCtx<'a>) -> futures::future::BoxFuture<'a, anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    use tauri::Emitter;
    let state = app.state::<AppState>();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = state.ai_jobs.lock().unwrap();
        if jobs.contains_key(&key) {
            return Err("Already running".into());
        }
        jobs.insert(key.clone(), cancel.clone());
    }
    let app = app.clone();
    Ok(tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let settings = settings::load(&state.settings_path());
        let emitter = app.clone();
        let on_event = move |e: ai::AiEvent| {
            let _ = emitter.emit("ai-event", e);
        };
        let ctx = ai::AiCtx { http: &state.http, db: &state.db, settings: &settings, cancel: &cancel, on_event: &on_event };
        let result = work(&ctx).await.map_err(|e| format!("{e:#}"));
        state.ai_jobs.lock().unwrap().remove(&key);
        result
    }))
}

/// Summarise a transcript in the background (progress on `ai-event`, key `digest:<id>`).
#[tauri::command]
fn ai_digest(app: AppHandle, id: String, force: bool) -> CmdResult<()> {
    let key = format!("digest:{id}");
    ai_run(&app, key, move |ctx| Box::pin(async move { ai::digest::digest_transcript(ctx, &id, force).await.map(|_| ()) }))?;
    Ok(())
}

/// Called when a transcription finishes: summarise it if the setting is on and it isn't current.
pub(crate) fn ai_auto_digest(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let s = settings::load(&state.settings_path());
    if !s.ai_auto_digest || s.providers.iter().find(|p| p.id == s.ai_provider).is_none_or(|p| p.api_key.trim().is_empty()) {
        return;
    }
    let id = id.to_string();
    let _ = ai_run(app, format!("digest:{id}"), move |ctx| Box::pin(async move { ai::digest::digest_transcript(ctx, &id, false).await.map(|_| ()) }));
}

#[tauri::command]
fn ai_get_digests(state: State<AppState>, id: String) -> CmdResult<Vec<ai::store::Digest>> {
    ai::store::digests_for(&state.db.lock().unwrap(), &id).map_err(err)
}

/// Keys of AI work currently running.
#[tauri::command]
fn ai_running(state: State<AppState>) -> Vec<String> {
    state.ai_jobs.lock().unwrap().keys().cloned().collect()
}

#[tauri::command]
fn ai_cancel(state: State<AppState>, key: String) {
    if let Some(c) = state.ai_jobs.lock().unwrap().get(&key) {
        c.store(true, Ordering::Relaxed);
    }
}

/// Ask a question. Returns the thread id at once; tokens stream on `ai-event` and the final
/// message (or error) arrives on `ai-answer`.
#[tauri::command]
fn ai_ask(app: AppHandle, state: State<AppState>, mut params: ai::ask::AskParams) -> CmdResult<String> {
    use tauri::Emitter;
    let thread_id = match &params.thread_id {
        Some(id) => id.clone(),
        None => {
            let db = state.db.lock().unwrap();
            let t = ai::store::create_thread(&db, params.question.trim(), &params.scope).map_err(err)?;
            t.id
        }
    };
    params.thread_id = Some(thread_id.clone());
    let tid = thread_id.clone();
    let app2 = app.clone();
    ai_run(&app, format!("ask:{thread_id}"), move |ctx| {
        Box::pin(async move {
            let r = ai::ask::ask(ctx, params).await;
            let payload = match &r {
                Ok(a) => serde_json::json!({ "thread_id": tid, "thread": a.thread, "message": a.message, "note": a.note, "error": null }),
                Err(e) => serde_json::json!({ "thread_id": tid, "error": format!("{e:#}") }),
            };
            let _ = app2.emit("ai-answer", payload);
            r.map(|_| ())
        })
    })?;
    Ok(thread_id)
}

#[tauri::command]
fn ai_list_threads(state: State<AppState>, scope: Option<ai::Scope>) -> CmdResult<Vec<ai::store::Thread>> {
    ai::store::list_threads(&state.db.lock().unwrap(), scope.as_ref()).map_err(err)
}

#[derive(serde::Serialize)]
struct ThreadDetail {
    thread: ai::store::Thread,
    messages: Vec<ai::store::Message>,
}

#[tauri::command]
fn ai_get_thread(state: State<AppState>, id: String) -> CmdResult<Option<ThreadDetail>> {
    let db = state.db.lock().unwrap();
    let Some(thread) = ai::store::get_thread(&db, &id).map_err(err)? else { return Ok(None) };
    let messages = ai::store::messages_for(&db, &id).map_err(err)?;
    Ok(Some(ThreadDetail { thread, messages }))
}

#[tauri::command]
fn ai_delete_thread(state: State<AppState>, id: String) -> CmdResult<()> {
    ai::store::delete_thread(&state.db.lock().unwrap(), &id).map_err(err)
}

/// Hybrid (words + meaning) search; the ⌘K dialog's second section.
#[tauri::command]
async fn ai_search(app: AppHandle, query: String, scope: ai::Scope, limit: usize) -> CmdResult<ai::retrieval::Retrieval> {
    let handle = ai_run(&app, format!("search:{}", uuid::Uuid::new_v4()), move |ctx| {
        Box::pin(async move { ai::retrieval::retrieve(ctx, &scope, &query, limit.clamp(1, 50)).await })
    })?;
    handle.await.map_err(err)?
}

/// Embed every transcript that isn't indexed yet (progress key `embed:library`).
#[tauri::command]
fn ai_embed_library(app: AppHandle) -> CmdResult<()> {
    ai_run(&app, "embed:library".into(), move |ctx| {
        Box::pin(async move {
            let ids: Vec<String> = ai::store::transcripts_in_scope(&ctx.db(), &ai::Scope::Library)?.into_iter().map(|t| t.id).collect();
            for id in &ids {
                ai::digest::ensure_chunks(ctx, id)?;
            }
            match ai::digest::embed_transcripts(ctx, &ids, "embed:library").await {
                Ok(n) => ctx.done("embed:library", format!("Indexed {n} part(s)"), None),
                Err(e) => ctx.done("embed:library", "", Some(format!("{e:#}"))),
            }
            Ok(())
        })
    })?;
    Ok(())
}

/// Project brief (markdown with citations) for a transcript, folder, tag or the library.
#[tauri::command]
async fn ai_brief(app: AppHandle, scope: ai::Scope) -> CmdResult<String> {
    let handle = ai_run(&app, format!("brief:{}", uuid::Uuid::new_v4()), move |ctx| Box::pin(async move { ai::brief::brief(ctx, &scope).await }))?;
    handle.await.map_err(err)?
}

/// English translation of a transcript (background; progress key `translate:<id>`, result via `ai_get_digests`).
#[tauri::command]
fn ai_translate(app: AppHandle, id: String, force: bool) -> CmdResult<()> {
    ai_run(&app, format!("translate:{id}"), move |ctx| Box::pin(async move { ai::translate::translate_transcript(ctx, &id, force).await.map(|_| ()) }))?;
    Ok(())
}

/// Write text to a file the user picked in a save dialog.
#[tauri::command]
fn save_text(path: String, text: String) -> CmdResult<()> {
    std::fs::write(path, text).map_err(err)
}

/// One tiny request to check a provider/model/key from Settings.
#[tauri::command]
async fn ai_test_connection(app: AppHandle, provider_id: String, model: String) -> CmdResult<String> {
    let handle = ai_run(&app, format!("test:{}", uuid::Uuid::new_v4()), move |ctx| {
        Box::pin(async move {
            let mut cfg = ctx.settings.providers.iter().find(|p| p.id == provider_id).cloned().ok_or_else(|| anyhow::anyhow!("Unknown provider"))?;
            if !model.trim().is_empty() {
                cfg.model = model.trim().to_string();
            }
            let system = [ai::chat::SystemBlock { text: "Reply with the single word OK.".into(), cache: false }];
            let msg = [ai::chat::ChatMessage { role: ai::chat::Role::User, content: "Ready?".into() }];
            let req = ai::chat::ChatRequest { system: &system, messages: &msg, max_tokens: 16, json: false, cached_content: None, temperature: Some(0.0) };
            let on_retry = |_m: String| {};
            let call = ai::chat::ChatCall { cancel: ctx.cancel, on_retry: &on_retry, on_delta: None };
            let r = ai::chat::complete(ctx.http, &cfg, &req, &call).await?;
            Ok(format!("{} replied: {} ({} in / {} out tokens)", cfg.model, r.text.trim(), r.usage.input_tokens, r.usage.output_tokens))
        })
    })?;
    handle.await.map_err(err)?
}

// Folders & tags -------------------------------------------------------------------

#[tauri::command]
fn list_folders(state: State<AppState>) -> CmdResult<Vec<db::Folder>> {
    db::list_folders(&state.db.lock().unwrap()).map_err(err)
}

#[tauri::command]
fn create_folder(state: State<AppState>, name: String, parent_id: Option<i64>) -> CmdResult<db::Folder> {
    db::create_folder(&state.db.lock().unwrap(), &name, parent_id).map_err(err)
}

#[tauri::command]
fn rename_folder(state: State<AppState>, id: i64, name: String) -> CmdResult<()> {
    db::rename_folder(&state.db.lock().unwrap(), id, &name).map_err(err)
}

#[tauri::command]
fn delete_folder(state: State<AppState>, id: i64) -> CmdResult<()> {
    db::delete_folder(&state.db.lock().unwrap(), id).map_err(err)
}

#[tauri::command]
fn list_tags(state: State<AppState>) -> CmdResult<Vec<db::Tag>> {
    db::list_tags(&state.db.lock().unwrap()).map_err(err)
}

#[tauri::command]
fn create_tag(state: State<AppState>, name: String, color: String) -> CmdResult<db::Tag> {
    db::create_tag(&state.db.lock().unwrap(), &name, &color).map_err(err)
}

#[tauri::command]
fn update_tag(state: State<AppState>, id: i64, name: String, color: String) -> CmdResult<()> {
    db::update_tag(&state.db.lock().unwrap(), id, &name, &color).map_err(err)
}

#[tauri::command]
fn delete_tag(state: State<AppState>, id: i64) -> CmdResult<()> {
    db::delete_tag(&state.db.lock().unwrap(), id).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let cache_dir = app.path().app_cache_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = db::open(&data_dir.join("library.sqlite"))?;
            db::fail_stale(&conn)?;
            db::backfill_content_hashes(&conn)?;
            let http = reqwest::Client::builder()
                // Long chunks (25 min with diarization) can take several minutes to process.
                .timeout(std::time::Duration::from_secs(1800))
                .build()?;
            app.manage(AppState {
                db: Mutex::new(conn),
                data_dir,
                cache_dir,
                http,
                cancels: Mutex::new(HashMap::new()),
                progress: Mutex::new(HashMap::new()),
                ai_jobs: Mutex::new(HashMap::new()),
            });
            // Overnight batches finish at Google while the app may be closed; keep checking.
            tauri::async_runtime::spawn(batch::poll_loop(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            default_chunk_minutes,
            start_transcription,
            retranscribe,
            find_problems,
            speech_in_gaps,
            playback_audio,
            audio_levels,
            diarize_transcript,
            diarization_info,
            repair_transcript,
            cancel_job,
            job_progress,
            check_batches,
            list_batches,
            list_transcripts,
            get_transcript,
            update_transcript,
            delete_transcript,
            trash_transcript,
            restore_transcript,
            empty_trash,
            set_flagged,
            rename_speaker,
            update_segment,
            search,
            export_transcript,
            list_versions,
            get_version,
            restore_version,
            delete_version,
            usage_summary,
            known_price,
            ai_digest,
            ai_get_digests,
            ai_running,
            ai_cancel,
            ai_ask,
            ai_list_threads,
            ai_get_thread,
            ai_delete_thread,
            ai_search,
            ai_embed_library,
            ai_test_connection,
            ai_brief,
            ai_translate,
            save_text,
            list_folders,
            create_folder,
            rename_folder,
            delete_folder,
            list_tags,
            create_tag,
            update_tag,
            delete_tag,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
