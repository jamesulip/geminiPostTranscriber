mod audio;
mod db;
mod export;
mod jobs;
mod providers;
mod settings;

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
    mut opts: jobs::JobOptions,
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
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancels.lock().unwrap().insert(id.clone(), cancel.clone());
    let job = jobs::Job { id, source, audio_path, provider, opts, cancel };
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

#[tauri::command]
fn cancel_job(state: State<AppState>, id: String) {
    if let Some(c) = state.cancels.lock().unwrap().get(&id) {
        c.store(true, Ordering::Relaxed);
    }
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

#[tauri::command]
fn delete_transcript(state: State<AppState>, id: String) -> CmdResult<()> {
    if let Some(c) = state.cancels.lock().unwrap().get(&id) {
        c.store(true, Ordering::Relaxed);
    }
    let audio = db::delete_transcript(&state.db.lock().unwrap(), &id).map_err(err)?;
    if let Some(p) = audio {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
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

/// Built-in price estimate (USD/min) for a model, shown as the Settings placeholder.
#[tauri::command]
fn known_price(model: String) -> f64 {
    settings::known_price(&model)
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
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(900))
                .build()?;
            app.manage(AppState {
                db: Mutex::new(conn),
                data_dir,
                cache_dir,
                http,
                cancels: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            default_chunk_minutes,
            start_transcription,
            retranscribe,
            cancel_job,
            list_transcripts,
            get_transcript,
            update_transcript,
            delete_transcript,
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
            list_folders,
            create_folder,
            rename_folder,
            delete_folder,
            list_tags,
            create_tag,
            delete_tag,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
