//! Background transcription jobs: preprocess → transcribe chunks → map times → store.

use crate::audio::{self, PreprocessOptions};
use crate::providers::{self, Request, Segment, TokenUsage, Transcription};
use crate::settings::{ProviderConfig, ProviderKind};
use crate::{db, quality, AppState};
use anyhow::{bail, Result};
use futures::future::{BoxFuture, FutureExt};
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOptions {
    pub provider_id: String,
    pub model: Option<String>,
    pub language: Option<String>,
    pub speed: f64,
    pub remove_silence: bool,
    pub silence_threshold_db: f64,
    pub silence_min_duration: f64,
    pub chunk_minutes: f64,
    #[serde(default = "default_method")]
    pub silence_method: crate::settings::SilenceMethod,
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f64,
    /// Send through the Gemini Batch API instead of transcribing now (cheaper, results within 24 h).
    #[serde(default)]
    pub batch: bool,
    pub title: Option<String>,
    pub folder_id: Option<i64>,
    #[serde(default)]
    pub tag_ids: Vec<i64>,
}

fn default_method() -> crate::settings::SilenceMethod {
    crate::settings::SilenceMethod::Speech
}

fn default_vad_threshold() -> f64 {
    0.5
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
    /// Re-transcribe only these stretches (original time) and splice them into the current version.
    pub repair: Option<Vec<audio::Interval>>,
}

pub fn emit(app: &AppHandle, id: &str, stage: &str, progress: f64, message: impl Into<String>) {
    let p = Progress { id: id.into(), stage: stage.into(), progress, message: message.into(), done: false, error: None };
    // Remember the latest status so a reloaded window can show it again.
    if let Some(state) = app.try_state::<AppState>() {
        state.progress.lock().unwrap().insert(id.to_string(), p.clone());
    }
    let _ = app.emit("job-progress", p);
}

/// What was actually sent to the provider, for usage/cost accounting.
#[derive(Default)]
pub struct Meter {
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

/// Finished chunk results kept on disk until the job succeeds, so a retry after a failure
/// (rate limit, network, app closed) only re-sends the chunks that didn't finish.
struct ChunkCache {
    dir: PathBuf,
    key: u64,
    /// Keys used by older versions of the app (which also hashed the language), so chunks saved
    /// before an update are still found.
    legacy: Vec<u64>,
}

impl ChunkCache {
    fn new(cache_dir: &std::path::Path, job: &Job) -> Self {
        let lang = job.opts.language.as_deref().unwrap_or("");
        let legacy = [lang, ""].iter().map(|l| Self::hash(job, Some(l))).collect();
        ChunkCache { dir: cache_dir.join("results").join(&job.id), key: Self::hash(job, None), legacy }
    }

    /// Only what changes the audio sent or the model matters; the language is just a hint, so
    /// adding/changing it on retry still reuses finished chunks.
    fn hash(job: &Job, legacy_language: Option<&str>) -> u64 {
        use std::hash::{Hash, Hasher};
        let o = &job.opts;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match legacy_language {
            Some(l) => (&job.provider.id, &job.provider.model, l).hash(&mut h),
            None => (&job.provider.id, &job.provider.model).hash(&mut h),
        }
        for v in [o.speed, o.silence_threshold_db, o.silence_min_duration, o.chunk_minutes, o.vad_threshold] {
            v.to_bits().hash(&mut h);
        }
        (o.remove_silence, format!("{:?}", o.silence_method)).hash(&mut h);
        std::fs::metadata(&job.audio_path).map(|m| m.len()).unwrap_or(0).hash(&mut h);
        h.finish()
    }

    fn file(&self, key: u64, i: usize, c: &audio::Chunk) -> PathBuf {
        self.dir.join(format!("{key:016x}-{i:04}-{:.3}-{:.3}.json", c.offset, c.duration))
    }

    fn path(&self, i: usize, c: &audio::Chunk) -> PathBuf {
        let current = self.file(self.key, i, c);
        if current.exists() {
            return current;
        }
        self.legacy.iter().map(|k| self.file(*k, i, c)).find(|p| p.exists()).unwrap_or(current)
    }

    fn get(&self, i: usize, c: &audio::Chunk) -> Option<Transcription> {
        serde_json::from_str(&std::fs::read_to_string(self.path(i, c)).ok()?).ok()
    }

    fn put(&self, i: usize, c: &audio::Chunk, t: &Transcription) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(json) = serde_json::to_string(t) {
            let _ = std::fs::write(self.file(self.key, i, c), json);
        }
    }

    fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub async fn run(app: AppHandle, job: Job) {
    let state = app.state::<AppState>();
    let work = state.cache_dir.join("jobs").join(&job.id);
    let meter = Mutex::new(Meter::default());
    let cache = ChunkCache::new(&state.cache_dir, &job);
    let result = match &job.repair {
        Some(ranges) => execute_repair(&app, &job, &work, &meter, ranges).await.map(|_| false),
        None => execute(&app, &job, &work, &meter, &cache).await,
    };
    // Submitted as a batch: Google finishes it later; `batch::poll` saves the result.
    if matches!(result, Ok(true)) {
        let _ = std::fs::remove_dir_all(&work);
        state.cancels.lock().unwrap().remove(&job.id);
        state.progress.lock().unwrap().remove(&job.id);
        let _ = app.emit(
            "job-progress",
            Progress {
                id: job.id.clone(),
                stage: "queued".into(),
                progress: 1.0,
                message: "Sent to Google as an overnight batch".into(),
                done: true,
                error: None,
            },
        );
        return;
    }
    let _ = std::fs::remove_dir_all(&work);
    if result.is_ok() {
        cache.clear();
    }
    state.cancels.lock().unwrap().remove(&job.id);
    state.progress.lock().unwrap().remove(&job.id);

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
            let original = m.sent_seconds * m.ratio;
            let (input, output) = (m.tokens.input_tokens as i64, m.tokens.output_tokens as i64);
            // Prefer actual token counts when the provider reports them and we know its token prices.
            let (cost_usd, saved_usd) = match job.provider.token_price() {
                Some(p) if input > 0 => db::token_cost(input, output, p, original, m.sent_seconds),
                _ => db::minute_cost(original, m.sent_seconds, job.provider.effective_price()),
            };
            let _ = db::insert_usage(
                &db,
                &db::NewUsage {
                    transcript_id: &job.id,
                    title: &title,
                    provider: &job.provider.id,
                    provider_name: &job.provider.name,
                    model: &job.provider.model,
                    status,
                    original_seconds: original,
                    sent_seconds: m.sent_seconds,
                    requests: m.requests,
                    input_tokens: input,
                    output_tokens: output,
                    price_per_minute: job.provider.effective_price(),
                    cost_usd,
                    saved_usd,
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

/// Returns `true` when the job was handed to Google as a batch (finished later by the poller).
async fn execute(
    app: &AppHandle,
    job: &Job,
    work: &std::path::Path,
    meter: &Mutex<Meter>,
    cache: &ChunkCache,
) -> Result<bool> {
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

    emit(app, id, "preprocess", 0.05, "Preparing audio");
    let o = &job.opts;
    let on_progress = |msg: &str, f: f64| emit(app, id, "preprocess", 0.05 + 0.1 * f, msg);
    let prepared = audio::preprocess(
        &job.audio_path,
        work,
        &PreprocessOptions {
            speed: o.speed,
            remove_silence: o.remove_silence,
            silence_threshold_db: o.silence_threshold_db,
            silence_min_duration: o.silence_min_duration,
            chunk_seconds: o.chunk_minutes * 60.0,
            method: o.silence_method.clone(),
            vad_threshold: o.vad_threshold,
        },
        &on_progress,
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
    let cached = prepared.chunks.iter().enumerate().filter(|(i, c)| cache.path(*i, c).exists()).count();
    let resume = if cached > 0 { format!(" · resuming, {cached} already done") } else { String::new() };
    emit(
        app,
        id,
        "transcribe",
        0.15,
        format!("Transcribing {n} chunk(s) · {saved:.0}% shorter than original{resume}"),
    );
    if cancelled() {
        bail!("Cancelled");
    }
    if o.batch {
        emit(app, id, "batch", 0.6, format!("Sending {n} chunk(s) to Google as an overnight batch"));
        crate::batch::submit(app, job, &prepared).await?;
        return Ok(true);
    }

    let language = o.language.as_deref().filter(|l| !l.trim().is_empty());
    let cut_points = prepared.map.boundaries();
    let raw_dir = state.data_dir.join("raw").join(id);
    let _ = std::fs::remove_dir_all(&raw_dir);
    std::fs::create_dir_all(&raw_dir)?;
    let ctx = ChunkCtx {
        client: &state.http,
        cfg: &job.provider,
        language,
        condensed: &prepared.condensed,
        cut_points: &cut_points,
        work,
        meter,
        raw_dir: &raw_dir,
        checks: quality::CheckOptions { condensed: o.remove_silence, want_speakers: job.provider.diarizes() },
        cancel: &job.cancel,
    };
    let notes: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let done = std::sync::atomic::AtomicUsize::new(0);
    // Finished chunks so far; saved as a partial transcript after each one, so a failed,
    // cancelled or interrupted job still shows what's done.
    let finished: Mutex<Vec<Option<Vec<Segment>>>> = Mutex::new(vec![None; n]);
    {
        let db = state.db.lock().unwrap();
        db::set_partial(&db, id, None)?;
    }
    let save_partial = |i: usize, segs: &[Segment]| {
        let json = {
            let mut f = finished.lock().unwrap();
            f[i] = Some(segs.to_vec());
            partial_json(&prepared.chunks, &f, &prepared.map)
        };
        let db = state.db.lock().unwrap();
        let _ = db::set_partial(&db, id, Some(&json));
    };
    let progress_now = || 0.15 + 0.8 * (done.load(Ordering::Relaxed) as f64 / n as f64);

    // One top-level chunk: cached result, or transcribe + check (+ split on problems).
    let run_chunk = |i: usize, chunk: audio::Chunk, speakers: Vec<String>, previous: String| {
        let (ctx, notes, done, save_partial) = (&ctx, &notes, &done, &save_partial);
        async move {
            if job.cancel.load(Ordering::Relaxed) {
                bail!("Cancelled");
            }
            let t = match cache.get(i, &chunk) {
                Some(t) => t,
                None => {
                    let on_retry = |msg: String| emit(app, id, "transcribe", progress_now(), msg);
                    let (t, problems) =
                        transcribe_checked(ctx, chunk.clone(), (i + 1).to_string(), speakers, previous, &on_retry, 0).await?;
                    notes.lock().unwrap().extend(problems);
                    cache.put(i, &chunk, &t);
                    t
                }
            };
            save_partial(i, &t.segments);
            let k = done.fetch_add(1, Ordering::Relaxed) + 1;
            emit(app, id, "transcribe", 0.15 + 0.8 * (k as f64 / n as f64), format!("Transcribed chunk {k}/{n}"));
            Ok::<_, anyhow::Error>(t.segments)
        }
    };

    let per_chunk: Vec<Vec<Segment>> = if providers::parallel(&job.provider) {
        let futs: Vec<_> = prepared
            .chunks
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, c)| run_chunk(i, c, Vec::new(), String::new()))
            .collect();
        futures::stream::iter(futs).buffered(3).try_collect().await?
    } else {
        // Sequential so each chunk can see the previous chunk's speakers and text.
        let mut all = Vec::with_capacity(n);
        let mut speakers: Vec<String> = Vec::new();
        let mut previous = String::new();
        for (i, chunk) in prepared.chunks.iter().enumerate() {
            let segs = run_chunk(i, chunk.clone(), speakers.clone(), previous.clone()).await?;
            for s in &segs {
                if let Some(sp) = &s.speaker {
                    if !speakers.contains(sp) {
                        speakers.push(sp.clone());
                    }
                }
            }
            previous = segs.iter().rev().take(4).rev().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
            all.push(segs);
        }
        all
    };
    let notes = notes.into_inner().unwrap();

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
            notes: (!notes.is_empty()).then(|| notes.join("\n")),
            processed_duration: prepared.processed_duration,
        },
    )?;
    Ok(false)
}

/// Re-transcribe only `ranges` of the original audio and splice the results into the current
/// version (which is archived first, like any re-transcription).
async fn execute_repair(
    app: &AppHandle,
    job: &Job,
    work: &std::path::Path,
    meter: &Mutex<Meter>,
    ranges: &[audio::Interval],
) -> Result<()> {
    let state = app.state::<AppState>();
    let id = job.id.as_str();
    let o = &job.opts;
    let current = {
        let db = state.db.lock().unwrap();
        db::get_transcript(&db, id)?.ok_or_else(|| anyhow::anyhow!("Transcript not found"))?
    };
    let total = current.summary.duration;
    let language = o.language.as_deref().filter(|l| !l.trim().is_empty());
    let raw_dir = state.data_dir.join("raw").join(id).join("repair");
    let _ = std::fs::remove_dir_all(&raw_dir);
    std::fs::create_dir_all(&raw_dir)?;
    std::fs::create_dir_all(work)?;

    let n = ranges.len();
    let (mut new_segs, mut notes) = (Vec::<Segment>::new(), Vec::<String>::new());
    let (mut original_sum, mut processed_sum) = (0.0, 0.0);
    for (r, range) in ranges.iter().enumerate() {
        if job.cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let (a, b) = (range.start.max(0.0), range.end.min(total));
        if b - a < 1.0 {
            continue;
        }
        // A little extra audio on each side so words at the edges aren't cut; only text that
        // starts inside [a, b) is used.
        let (ca, cb) = ((a - 2.0).max(0.0), (b + 2.0).min(total));
        let label = format!("{}/{n} ({}–{})", r + 1, crate::export::clock(a), crate::export::clock(b));
        let base = 0.05 + 0.9 * r as f64 / n as f64;
        emit(app, id, "transcribe", base, format!("Fixing part {label}"));

        let slice = work.join(format!("range_{r}.wav"));
        audio::extract_range(&job.audio_path, ca, cb - ca, &slice).await?;
        let rwork = work.join(format!("range_{r}"));
        let on_progress = |msg: &str, f: f64| emit(app, id, "preprocess", base + 0.1 * f / n as f64, format!("Part {label}: {msg}"));
        let prepared = match audio::preprocess(&slice, &rwork, &preprocess_options(o), &on_progress).await {
            Ok(p) => p,
            Err(e) if e.to_string().starts_with("No speech found") => {
                notes.push(format!("Part {label}: no speech found"));
                continue;
            }
            Err(e) => return Err(e),
        };
        original_sum += prepared.original_duration;
        processed_sum += prepared.processed_duration;
        meter.lock().unwrap().ratio = original_sum / processed_sum.max(0.001);

        let cut_points = prepared.map.boundaries();
        let ctx = ChunkCtx {
            client: &state.http,
            cfg: &job.provider,
            language,
            condensed: &prepared.condensed,
            cut_points: &cut_points,
            work: &rwork,
            meter,
            raw_dir: &raw_dir,
            checks: quality::CheckOptions { condensed: o.remove_silence, want_speakers: job.provider.diarizes() },
            cancel: &job.cancel,
        };
        let on_retry = |msg: String| emit(app, id, "transcribe", base, format!("Part {label}: {msg}"));
        let mut per_chunk = Vec::new();
        let mut speakers: Vec<String> = Vec::new();
        let mut previous = String::new();
        for (i, chunk) in prepared.chunks.iter().enumerate() {
            let (t, problems) = transcribe_checked(
                &ctx, chunk.clone(), format!("r{}.{}", r + 1, i + 1), speakers.clone(), previous.clone(), &on_retry, 0,
            )
            .await?;
            notes.extend(problems);
            for s in t.segments.iter().filter_map(|s| s.speaker.clone()) {
                if !speakers.contains(&s) {
                    speakers.push(s);
                }
            }
            previous = t.segments.iter().rev().take(4).rev().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
            per_chunk.push(t.segments);
        }
        new_segs.extend(
            merge(&prepared.chunks, per_chunk, &prepared.map)
                .into_iter()
                .map(|s| Segment { start: s.start + ca, end: s.end + ca, ..s })
                .filter(|s| s.start >= a && s.start < b),
        );
    }

    // Keep everything outside the repaired stretches (minus impossible "lines"), add the new text.
    emit(app, id, "merge", 0.97, "Merging");
    let combined = splice(&current.segments, ranges, new_segs);

    let detected = meter.lock().unwrap().language.clone();
    let mut db = state.db.lock().unwrap();
    db::commit_version_with_speakers(
        &mut db,
        id,
        &combined,
        &db::CompletedVersion {
            provider: &job.provider.id,
            model: &job.provider.model,
            language: language.or(current.summary.language.as_deref()),
            detected_language: detected.as_deref(),
            notes: (!notes.is_empty()).then(|| notes.join("\n")),
            processed_duration: current.summary.processed_duration,
        },
        &current.speakers,
    )?;
    Ok(())
}

/// Old segments outside `ranges` (minus impossible multi-minute "lines") + the new ones, sorted,
/// with loops removed.
pub fn splice(old: &[db::SegmentRow], ranges: &[audio::Interval], new: Vec<Segment>) -> Vec<Segment> {
    let inside = |t: f64| ranges.iter().any(|r| t >= r.start && t < r.end);
    let mut combined: Vec<Segment> = old
        .iter()
        .filter(|s| !inside(s.start) && s.end - s.start <= 120.0)
        .map(|s| Segment { start: s.start, end: s.end, text: s.text.clone(), speaker: s.speaker.clone() })
        .collect();
    combined.extend(new.into_iter().filter(|s| inside(s.start)));
    combined.sort_by(|a, b| a.start.total_cmp(&b.start));
    quality::remove_loops(combined).0
}

fn preprocess_options(o: &JobOptions) -> PreprocessOptions {
    PreprocessOptions {
        speed: o.speed,
        remove_silence: o.remove_silence,
        silence_threshold_db: o.silence_threshold_db,
        silence_min_duration: o.silence_min_duration,
        chunk_seconds: o.chunk_minutes * 60.0,
        method: o.silence_method.clone(),
        vad_threshold: o.vad_threshold,
    }
}

/// A chunk whose transcript fails the quality check is re-cut into halves this many times at most.
const MAX_SPLIT_DEPTH: u32 = 2;
/// Don't split chunks shorter than this.
const MIN_SPLIT_SECS: f64 = 120.0;

/// Everything needed to transcribe (and re-cut) chunks of one job.
pub struct ChunkCtx<'a> {
    pub client: &'a reqwest::Client,
    pub cfg: &'a ProviderConfig,
    pub language: Option<&'a str>,
    pub condensed: &'a std::path::Path,
    pub cut_points: &'a [f64],
    pub work: &'a std::path::Path,
    pub meter: &'a Mutex<Meter>,
    /// Raw provider responses are saved here for diagnosing bad transcripts.
    pub raw_dir: &'a std::path::Path,
    pub checks: quality::CheckOptions,
    /// The job's Cancel flag.
    pub cancel: &'a AtomicBool,
}

/// Transcribe a chunk and check the result. On problems (skipped speech, loops, impossible timings,
/// missing speakers) re-transcribe it as two halves, which long-audio failures usually fix.
/// Returns the transcript (chunk-relative) and notes about problems that remained.
pub fn transcribe_checked<'a>(
    ctx: &'a ChunkCtx<'a>,
    chunk: audio::Chunk,
    label: String,
    speakers: Vec<String>,
    previous: String,
    on_retry: &'a (dyn Fn(String) + Send + Sync),
    depth: u32,
) -> BoxFuture<'a, Result<(Transcription, Vec<String>)>> {
    async move {
        let req = Request {
            path: &chunk.path,
            duration: chunk.duration,
            language: ctx.language,
            known_speakers: &speakers,
            previous_text: &previous,
            on_retry,
            cancel: ctx.cancel,
        };
        let mut t = providers::transcribe(ctx.client, ctx.cfg, &req).await?;
        ctx.meter.lock().unwrap().add(chunk.duration, t.usage, t.language.clone());
        if let Some(raw) = t.raw.take() {
            let _ = std::fs::write(ctx.raw_dir.join(format!("chunk-{label}.json")), raw.to_string());
        }
        let Some(problem) = quality::check(&t.segments, chunk.duration, &ctx.checks) else {
            t.segments = quality::remove_loops(t.segments).0;
            return Ok((t, Vec::new()));
        };
        if depth >= MAX_SPLIT_DEPTH || chunk.duration < MIN_SPLIT_SECS {
            // Keep what we have, minus loops, and tell the user this part may be incomplete.
            t.segments = quality::remove_loops(t.segments).0;
            return Ok((t, vec![format!("Part {label} ({}): {problem}", span(&chunk))]));
        }

        on_retry(format!("Chunk {label}: {problem} — re-transcribing it in two halves"));
        let mid = audio::split_point(ctx.cut_points, chunk.offset, chunk.offset + chunk.duration);
        let end = chunk.offset + chunk.duration;
        let a = audio::encode_chunk(ctx.condensed, chunk.offset, mid - chunk.offset, &ctx.work.join(format!("chunk_{label}a.ogg"))).await?;
        let b = audio::encode_chunk(ctx.condensed, mid, end - mid, &ctx.work.join(format!("chunk_{label}b.ogg"))).await?;
        let (ta, mut notes) =
            transcribe_checked(ctx, a.clone(), format!("{label}a"), speakers.clone(), previous, on_retry, depth + 1).await?;
        let mut more = speakers;
        for s in ta.segments.iter().filter_map(|s| s.speaker.clone()) {
            if !more.contains(&s) {
                more.push(s);
            }
        }
        let tail = ta.segments.iter().rev().take(4).rev().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
        let (tb, nb) = transcribe_checked(ctx, b.clone(), format!("{label}b"), more, tail, on_retry, depth + 1).await?;
        notes.extend(nb);

        // Halves are relative to their own start; shift into this chunk's time.
        let shift = b.offset - chunk.offset;
        let mut segments = ta.segments;
        segments.extend(tb.segments.into_iter().map(|s| Segment { start: s.start + shift, end: s.end + shift, ..s }));
        let mut usage = ta.usage;
        usage += tb.usage;
        Ok((Transcription { segments, usage, language: ta.language.or(tb.language), raw: None }, notes))
    }
    .boxed()
}

fn span(c: &audio::Chunk) -> String {
    format!("{:.0}–{:.0} min of sent audio", c.offset / 60.0, (c.offset + c.duration) / 60.0)
}

/// The finished chunks so far, in original time, as JSON: `{done, total, segments}`.
pub fn partial_json(chunks: &[audio::Chunk], finished: &[Option<Vec<Segment>>], map: &audio::TimeMap) -> String {
    let (done_chunks, done_segs): (Vec<audio::Chunk>, Vec<Vec<Segment>>) = chunks
        .iter()
        .zip(finished)
        .filter_map(|(c, f)| f.as_ref().map(|s| (c.clone(), s.clone())))
        .unzip();
    let segments = quality::remove_loops(merge(&done_chunks, done_segs, map)).0;
    serde_json::json!({ "done": done_chunks.len(), "total": chunks.len(), "segments": segments }).to_string()
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

    #[test]
    fn partial_json_maps_only_finished_chunks() {
        let map = audio::TimeMap::new(&[audio::Interval { start: 0.0, end: 100.0 }, audio::Interval { start: 200.0, end: 300.0 }], 1.0);
        let chunk = |o: f64| audio::Chunk { path: PathBuf::new(), offset: o, duration: 100.0 };
        let seg = |t: &str| Segment { start: 10.0, end: 12.0, text: t.into(), speaker: None };
        let json: serde_json::Value =
            serde_json::from_str(&partial_json(&[chunk(0.0), chunk(100.0)], &[None, Some(vec![seg("second chunk")])], &map)).unwrap();
        assert_eq!((json["done"].as_u64(), json["total"].as_u64()), (Some(1), Some(2)));
        // Chunk 2 starts at processed 100 s = original 200 s.
        assert_eq!(json["segments"][0]["start"].as_f64(), Some(210.0));
        assert_eq!(json["segments"][0]["text"], "second chunk");
    }

    #[test]
    fn splice_replaces_only_the_repaired_ranges() {
        let row = |i: i64, a: f64, b: f64, t: &str| db::SegmentRow { id: i, idx: i, start: a, end: b, speaker: None, text: t.into() };
        let old = vec![
            row(1, 10.0, 12.0, "before"),
            row(2, 100.0, 101.0, "loop line here"),
            row(3, 100.0, 101.0, "loop line here"),
            row(4, 150.0, 1900.0, "giant"),
            row(5, 300.0, 302.0, "after"),
        ];
        let new = vec![
            Segment { start: 95.0, end: 99.0, text: "outside, from the padding".into(), speaker: None },
            Segment { start: 120.0, end: 125.0, text: "recovered speech".into(), speaker: Some("Speaker 1".into()) },
        ];
        let out = splice(&old, &[audio::Interval { start: 100.0, end: 200.0 }], new);
        let texts: Vec<&str> = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["before", "recovered speech", "after"]);
    }

    /// Full path against a local mock server: `MOCK_URL=http://127.0.0.1:8765 MOCK_AUDIO=... cargo test -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn end_to_end_with_mock_provider() {
        let url = std::env::var("MOCK_URL").unwrap();
        let input = PathBuf::from(std::env::var("MOCK_AUDIO").unwrap());
        let work = std::env::temp_dir().join(format!("tx-e2e-{}", uuid::Uuid::new_v4()));
        let chunk_seconds = std::env::var("MOCK_CHUNK").ok().and_then(|v| v.parse().ok()).unwrap_or(30.0);
        let opts = PreprocessOptions { speed: 1.25, remove_silence: true, silence_threshold_db: -35.0, silence_min_duration: 0.7, chunk_seconds, method: crate::settings::SilenceMethod::Speech, vad_threshold: 0.5 };
        let prepared = audio::preprocess(&input, &work, &opts, &|_, _| {}).await.unwrap();
        let cfg = ProviderConfig { id: "mock".into(), name: "Mock".into(), kind: ProviderKind::Openai, base_url: url, api_key: "test".into(), model: "whisper-mock".into(), price_per_minute: None, price_input_per_mtok: None, price_output_per_mtok: None };
        let client = reqwest::Client::new();
        let meter = Mutex::new(Meter::default());
        let cut_points = prepared.map.boundaries();
        let raw_dir = work.join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let never = AtomicBool::new(false);
        let ctx = ChunkCtx {
            client: &client, cfg: &cfg, language: None, condensed: &prepared.condensed, cut_points: &cut_points,
            work: &work, meter: &meter, raw_dir: &raw_dir,
            checks: quality::CheckOptions { condensed: true, want_speakers: false }, cancel: &never,
        };
        let on_retry = |m: String| println!("{m}");
        let mut per_chunk = vec![];
        for (i, c) in prepared.chunks.iter().enumerate() {
            let (t, notes) = transcribe_checked(&ctx, c.clone(), (i + 1).to_string(), vec![], String::new(), &on_retry, 0).await.unwrap();
            for n in notes { println!("note: {n}"); }
            per_chunk.push(t.segments);
        }
        println!("requests {}", meter.lock().unwrap().requests);
        let merged = merge(&prepared.chunks, per_chunk, &prepared.map);
        println!("orig {:.2}s processed {:.2}s chunks {}", prepared.original_duration, prepared.processed_duration, prepared.chunks.len());
        for s in &merged { println!("{:7.2} {:7.2} {}", s.start, s.end, s.text); }
        let _ = std::fs::remove_dir_all(work);
    }
}

