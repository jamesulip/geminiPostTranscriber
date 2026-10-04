//! Local speaker diarization with sherpa-onnx: pyannote segmentation 3.0 finds speaker turns,
//! 3D-Speaker ERes2Net embeddings cluster them into speakers. Runs on the CPU, no account or key.
//!
//! The current transcript's segments get the diarizer's speaker with the most overlap; the
//! previous labeling is archived as a version so it can be restored.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

use crate::db::{self, Speaker};
use crate::{audio, jobs, AppState};

const RELEASES: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download";
const SEGMENTATION_ARCHIVE: &str = "sherpa-onnx-pyannote-segmentation-3-0.tar.bz2";
const SEGMENTATION_MODEL: &str = "sherpa-onnx-pyannote-segmentation-3-0/model.onnx";
/// Best of the embedding models tried on real meeting audio (see scripts/diarization-spike).
pub const EMBEDDING_MODEL: &str = "3dspeaker_speech_eres2net_base_200k_sv_zh-cn_16k-common.onnx";
/// Throughput to assume for the progress estimate before the first run has been measured.
const DEFAULT_REALTIME_FACTOR: f64 = 10.0;

/// Measured audio-seconds per wall-second from the last run, so the next estimate fits this machine.
fn rate_file(data_dir: &Path) -> PathBuf {
    models_dir(data_dir).join("rate.txt")
}
pub fn load_rate(data_dir: &Path) -> f64 {
    std::fs::read_to_string(rate_file(data_dir)).ok().and_then(|s| s.trim().parse().ok()).filter(|r: &f64| *r > 0.5).unwrap_or(DEFAULT_REALTIME_FACTOR)
}

/// While this is alive the process is marked as doing user-initiated work: macOS then keeps it
/// off App Nap and lets its threads run on the performance cores even when the window is in the
/// background. Without it a 90-minute recording took 23 min instead of 9.
struct BusyGuard {
    #[cfg(target_os = "macos")]
    activity: objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>,
}

// SAFETY: the activity token is only handed back to `endActivity`, which Apple documents as
// callable from any thread; nothing else touches it.
unsafe impl Send for BusyGuard {}

impl BusyGuard {
    fn begin(reason: &str) -> Self {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
            let options = NSActivityOptions::UserInitiated | NSActivityOptions::LatencyCritical;
            let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(options, &NSString::from_str(reason));
            Self { activity }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reason;
            Self {}
        }
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        unsafe { objc2_foundation::NSProcessInfo::processInfo().endActivity(&self.activity) };
    }
}

/// Ask macOS to schedule the calling thread as user-initiated work (performance cores).
fn raise_thread_priority() {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0);
    }
}
pub const STAGE: &str = "diarize";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: i32,
}

pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

fn model_paths(data_dir: &Path) -> (PathBuf, PathBuf) {
    let dir = models_dir(data_dir);
    (dir.join(SEGMENTATION_MODEL), dir.join(EMBEDDING_MODEL))
}

pub fn models_ready(data_dir: &Path) -> bool {
    let (seg, emb) = model_paths(data_dir);
    seg.is_file() && emb.is_file()
}

/// Stream a download to `dest` (via a `.part` file), reporting bytes so far.
async fn download(http: &reqwest::Client, url: &str, dest: &Path, cancel: &AtomicBool, mut on_progress: impl FnMut(u64, Option<u64>)) -> Result<()> {
    let mut resp = http.get(url).send().await.with_context(|| format!("downloading {url}"))?.error_for_status()?;
    let total = resp.content_length();
    let tmp = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut got = 0u64;
    while let Some(chunk) = resp.chunk().await? {
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        got += chunk.len() as u64;
        on_progress(got, total);
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

/// Download the models on first use (about 45 MB). Returns (segmentation, embedding) paths.
pub async fn ensure_models(app: &AppHandle, id: &str, data_dir: &Path, http: &reqwest::Client, cancel: &AtomicBool) -> Result<(PathBuf, PathBuf)> {
    let dir = models_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let (seg, emb) = model_paths(data_dir);
    let mb = |n: u64| format!("{:.0} MB", n as f64 / 1e6);
    if !seg.is_file() {
        let archive = dir.join(SEGMENTATION_ARCHIVE);
        let url = format!("{RELEASES}/speaker-segmentation-models/{SEGMENTATION_ARCHIVE}");
        download(http, &url, &archive, cancel, |got, total| {
            jobs::emit(app, id, STAGE, 0.02, format!("Downloading the speaker segmentation model ({} of {})…", mb(got), total.map(mb).unwrap_or("?".into())));
        })
        .await?;
        // The archive holds a folder with model.onnx plus scripts; the system tar handles .tar.bz2 on macOS, Linux and Windows 10+.
        let status = tokio::process::Command::new("tar")
            .arg("xjf")
            .arg(&archive)
            .arg("-C")
            .arg(&dir)
            .status()
            .await
            .context("running tar")?;
        let _ = std::fs::remove_file(&archive);
        if !status.success() || !seg.is_file() {
            bail!("Couldn't unpack the segmentation model");
        }
    }
    if !emb.is_file() {
        let url = format!("{RELEASES}/speaker-recongition-models/{EMBEDDING_MODEL}");
        download(http, &url, &emb, cancel, |got, total| {
            jobs::emit(app, id, STAGE, 0.05, format!("Downloading the speaker embedding model ({} of {})…", mb(got), total.map(mb).unwrap_or("?".into())));
        })
        .await?;
    }
    Ok((seg, emb))
}

/// Run the diarizer on a 16 kHz mono WAV. Blocking; call from `spawn_blocking`.
pub fn diarize_wav(seg_model: &Path, emb_model: &Path, wav: &Path, num_speakers: Option<i32>) -> Result<Vec<Turn>> {
    use sherpa_onnx::*;
    let threads = std::env::var("DIAR_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(2).clamp(2, 8) as i32
    });
    let provider = std::env::var("DIAR_PROVIDER").ok();
    let config = OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig { model: Some(seg_model.to_string_lossy().into()), ..Default::default() },
            num_threads: threads,
            provider: provider.clone().or(Some("cpu".into())),
            ..Default::default()
        },
        embedding: SpeakerEmbeddingExtractorConfig { model: Some(emb_model.to_string_lossy().into()), num_threads: threads, provider: provider.or(Some("cpu".into())), ..Default::default() },
        // With a known speaker count the clustering is reliable; the threshold mode over-splits on
        // phone recordings (50-200 "speakers" on a 4-person meeting), so it's only a fallback.
        clustering: FastClusteringConfig { num_clusters: num_speakers.unwrap_or(-1), threshold: 0.9, ..Default::default() },
        min_duration_on: 0.3,
        min_duration_off: 0.5,
    };
    let sd = OfflineSpeakerDiarization::create(&config).ok_or_else(|| anyhow!("Couldn't load the diarization models"))?;
    let wave = sherpa_onnx::Wave::read(&wav.to_string_lossy()).ok_or_else(|| anyhow!("Couldn't read the decoded audio"))?;
    if wave.sample_rate() != sd.sample_rate() {
        bail!("Decoded audio is {} Hz, the model needs {} Hz", wave.sample_rate(), sd.sample_rate());
    }
    raise_thread_priority();
    let result = sd.process(wave.samples()).ok_or_else(|| anyhow!("Diarization failed"))?;
    Ok(result
        .sort_by_start_time()
        .into_iter()
        .map(|s| Turn { start: s.start as f64, end: s.end as f64, speaker: s.speaker })
        .collect())
}

fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// The diarizer speaker covering most of `[start, end]`, if it covers enough of it to trust.
fn dominant(turns: &[Turn], start: f64, end: f64) -> Option<i32> {
    let mut share: HashMap<i32, f64> = HashMap::new();
    for t in turns {
        if t.end < start {
            continue;
        }
        if t.start > end {
            break;
        }
        *share.entry(t.speaker).or_default() += overlap(start, end, t.start, t.end);
    }
    let (spk, v) = share.into_iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    (v >= 0.3 * (end - start).max(0.1) || v >= 1.0).then_some(spk)
}

pub struct Assignment {
    /// Wall-clock seconds the diarizer itself took (0 when unknown).
    pub seconds: f64,
    /// Segment id → new label (None: no diarizer speech under it).
    pub labels: Vec<(i64, Option<String>)>,
    /// Labels in order of first appearance, with display names carried over where possible.
    pub speakers: Vec<Speaker>,
}

/// Map turns onto segments. Names from the previous labeling follow the cluster they overlapped
/// most (one name per cluster, biggest overlaps first); default "Speaker N" names don't carry.
pub fn assign(turns: &[Turn], segments: &[db::SegmentRow], old_names: &[Speaker]) -> Assignment {
    let names: HashMap<&str, &str> = old_names.iter().map(|s| (s.label.as_str(), s.display_name.as_str())).collect();
    let mut clusters: Vec<(i64, Option<i32>)> = Vec::with_capacity(segments.len());
    let mut votes: HashMap<(i32, String), f64> = HashMap::new();
    for s in segments {
        let c = dominant(turns, s.start, s.end);
        clusters.push((s.id, c));
        if let (Some(c), Some(old)) = (c, s.speaker.as_deref()) {
            let name = names.get(old).copied().unwrap_or(old);
            if name != old && !name.starts_with("Speaker ") {
                *votes.entry((c, name.to_string())).or_default() += s.end - s.start;
            }
        }
    }
    let mut order: Vec<i32> = Vec::new();
    for (_, c) in &clusters {
        if let Some(c) = c {
            if !order.contains(c) {
                order.push(*c);
            }
        }
    }
    let label = |c: i32| format!("Speaker {}", order.iter().position(|&x| x == c).unwrap() + 1);
    let mut ranked: Vec<_> = votes.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut carried: HashMap<i32, String> = HashMap::new();
    for ((c, name), _) in ranked {
        if carried.contains_key(&c) || carried.values().any(|n| *n == name) {
            continue;
        }
        carried.insert(c, name);
    }
    Assignment {
        seconds: 0.0,
        labels: clusters.into_iter().map(|(id, c)| (id, c.map(label))).collect(),
        speakers: order.iter().map(|&c| Speaker { label: label(c), display_name: carried.get(&c).cloned().unwrap_or_else(|| label(c)) }).collect(),
    }
}

/// The whole job: models → decode → diarize → apply. Progress goes out as a "diarize" job.
pub async fn run(app: AppHandle, id: String, num_speakers: Option<i32>, cancel: Arc<AtomicBool>) -> Result<Assignment> {
    let state = app.state::<AppState>();
    let (audio_path, duration, segments, speakers) = {
        let db = state.db.lock().unwrap();
        let t = db::get_transcript(&db, &id)?.ok_or_else(|| anyhow!("Transcript not found"))?;
        if t.segments.is_empty() {
            bail!("Nothing to label yet: transcribe the recording first");
        }
        (PathBuf::from(t.audio_path), t.summary.duration, t.segments, t.speakers)
    };
    jobs::emit(&app, &id, STAGE, 0.01, "Checking the speaker models…");
    let (seg, emb) = ensure_models(&app, &id, &state.data_dir, &state.http, &cancel).await?;

    jobs::emit(&app, &id, STAGE, 0.08, "Decoding the audio…");
    std::fs::create_dir_all(&state.cache_dir)?;
    let wav = state.cache_dir.join(format!("diarize-{id}.wav"));
    audio::decode_wav16k(&audio_path, &wav).await?;
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&wav);
        bail!("Cancelled");
    }

    // No progress callback from the library: estimate from the audio length and the last measured speed.
    let _busy = BusyGuard::begin("Finding speakers");
    let expected = Duration::from_secs_f64((duration.max(60.0) / load_rate(&state.data_dir)).max(10.0));
    let started = Instant::now();
    let ticker = {
        let (app, id) = (app.clone(), id.clone());
        tauri::async_runtime::spawn(async move {
            loop {
                let frac = (started.elapsed().as_secs_f64() / expected.as_secs_f64()).min(1.0);
                let left = expected.saturating_sub(started.elapsed()).as_secs();
                let msg = if left > 0 {
                    format!("Finding speakers (about {} left)…", if left >= 60 { format!("{} min", left.div_ceil(60)) } else { format!("{left} s") })
                } else {
                    format!("Finding speakers ({} min so far, longer than expected)…", started.elapsed().as_secs() / 60)
                };
                jobs::emit(&app, &id, STAGE, 0.1 + 0.85 * frac, msg);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        })
    };
    let turns = {
        let (seg, emb, wav) = (seg.clone(), emb.clone(), wav.clone());
        tokio::task::spawn_blocking(move || diarize_wav(&seg, &emb, &wav, num_speakers)).await?
    };
    ticker.abort();
    let _ = std::fs::remove_file(&wav);
    let turns = turns?;
    let elapsed = started.elapsed().as_secs_f64();
    if duration > 60.0 && elapsed > 1.0 {
        let _ = std::fs::write(rate_file(&state.data_dir), format!("{:.2}", duration / elapsed));
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }

    jobs::emit(&app, &id, STAGE, 0.97, "Assigning speakers to the transcript…");
    let mut assignment = assign(&turns, &segments, &speakers);
    assignment.seconds = elapsed;
    {
        let mut db = state.db.lock().unwrap();
        db::apply_speakers(&mut db, &id, &assignment.labels, &assignment.speakers)?;
        db::save_diarization(&db, &id, EMBEDDING_MODEL, num_speakers, &serde_json::to_string(&turns)?)?;
    }
    Ok(assignment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: i64, start: f64, end: f64, speaker: Option<&str>) -> db::SegmentRow {
        db::SegmentRow { id, idx: id, start, end, speaker: speaker.map(String::from), text: String::new() }
    }

    #[test]
    fn assigns_dominant_speaker_and_carries_names() {
        let turns = vec![
            Turn { start: 0.0, end: 5.0, speaker: 7 },
            Turn { start: 5.0, end: 9.0, speaker: 3 },
            Turn { start: 9.0, end: 10.0, speaker: 7 },
        ];
        let segments = vec![
            seg(1, 0.0, 4.0, Some("Speaker 1")),   // cluster 7, previously "Ana"
            seg(2, 4.5, 8.5, Some("Speaker 2")),   // cluster 3, previously default name → not carried
            seg(3, 8.0, 10.0, Some("Speaker 1")),  // mostly cluster 3 (1 s of 7 vs 1 s of 3 → tie broken by max; either way labeled)
            seg(4, 20.0, 25.0, Some("Speaker 1")), // no turns → unlabeled
        ];
        let names = vec![Speaker { label: "Speaker 1".into(), display_name: "Ana".into() }, Speaker { label: "Speaker 2".into(), display_name: "Speaker 2".into() }];
        let a = assign(&turns, &segments, &names);
        assert_eq!(a.labels[0], (1, Some("Speaker 1".into())));
        assert_eq!(a.labels[1], (2, Some("Speaker 2".into())));
        assert_eq!(a.labels[3], (4, None));
        assert_eq!(a.speakers[0], Speaker { label: "Speaker 1".into(), display_name: "Ana".into() });
        assert_eq!(a.speakers[1].display_name, "Speaker 2");
    }

    /// Timing harness: `DIAR_WAV=… DIAR_MODELS=… cargo test --release time_diarize -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn time_diarize() {
        let models = PathBuf::from(std::env::var("DIAR_MODELS").unwrap());
        let wav = PathBuf::from(std::env::var("DIAR_WAV").unwrap());
        let t0 = Instant::now();
        let (seg, emb) = (models.join(SEGMENTATION_MODEL), models.join(EMBEDDING_MODEL));
        let turns = if std::env::var("DIAR_IN_BLOCKING").is_ok() {
            // Same path as the app: a tokio blocking thread.
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move { tokio::task::spawn_blocking(move || diarize_wav(&seg, &emb, &wav, Some(3))).await }).unwrap().unwrap()
        } else {
            diarize_wav(&seg, &emb, &wav, Some(3)).unwrap()
        };
        eprintln!("TIMING {:.1}s for {} turns", t0.elapsed().as_secs_f64(), turns.len());
    }

    #[test]
    fn short_overlap_is_not_enough() {
        let turns = vec![Turn { start: 0.0, end: 0.5, speaker: 0 }];
        assert_eq!(dominant(&turns, 0.0, 10.0), None);
        assert_eq!(dominant(&turns, 0.0, 1.0), Some(0));
    }
}
