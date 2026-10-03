//! Audio preprocessing: normalize, detect/remove silence, speed up, chunk.
//! Keeps a time map so transcript timestamps can be mapped back to the original file.

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::settings::SilenceMethod;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    pub start: f64,
    pub end: f64,
}

/// A kept stretch of original audio, placed at `processed_start` in the condensed audio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Piece {
    pub processed_start: f64,
    pub original_start: f64,
    pub original_duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeMap {
    pub pieces: Vec<Piece>,
    pub speed: f64,
}

impl TimeMap {
    pub fn new(keeps: &[Interval], speed: f64) -> Self {
        let mut pieces = Vec::with_capacity(keeps.len());
        let mut p = 0.0;
        for k in keeps {
            let dur = k.end - k.start;
            pieces.push(Piece { processed_start: p, original_start: k.start, original_duration: dur });
            p += dur / speed;
        }
        TimeMap { pieces, speed }
    }

    pub fn processed_duration(&self) -> f64 {
        self.pieces
            .last()
            .map(|l| l.processed_start + l.original_duration / self.speed)
            .unwrap_or(0.0)
    }

    /// Processed time → original time. A time exactly on a piece boundary maps to the
    /// start of the later piece (good for segment starts).
    pub fn to_original(&self, t: f64) -> f64 {
        self.map(t, false)
    }

    /// Like `to_original` but a boundary maps to the end of the earlier piece
    /// (good for segment ends, so they don't stretch over removed silence).
    pub fn to_original_end(&self, t: f64) -> f64 {
        self.map(t, true)
    }

    fn map(&self, t: f64, prefer_earlier: bool) -> f64 {
        if self.pieces.is_empty() {
            return t * self.speed;
        }
        let t = t.max(0.0);
        let idx = self
            .pieces
            .iter()
            .rposition(|p| if prefer_earlier { p.processed_start < t } else { p.processed_start <= t })
            .unwrap_or(0);
        let p = &self.pieces[idx];
        let offset = ((t - p.processed_start) * self.speed).clamp(0.0, p.original_duration);
        p.original_start + offset
    }

    /// Processed-time positions where cutting doesn't split speech (piece boundaries).
    pub fn boundaries(&self) -> Vec<f64> {
        self.pieces.iter().skip(1).map(|p| p.processed_start).collect()
    }
}

/// Parse `silencedetect` output from ffmpeg stderr.
pub fn parse_silences(stderr: &str, total: f64) -> Vec<Interval> {
    let re_start = Regex::new(r"silence_start:\s*(-?[\d.]+)").unwrap();
    let re_end = Regex::new(r"silence_end:\s*(-?[\d.]+)").unwrap();
    let mut out = Vec::new();
    let mut open: Option<f64> = None;
    for line in stderr.lines() {
        if let Some(c) = re_start.captures(line) {
            open = c[1].parse::<f64>().ok().map(|v| v.max(0.0));
        } else if let Some(c) = re_end.captures(line) {
            if let (Some(s), Ok(e)) = (open.take(), c[1].parse::<f64>()) {
                out.push(Interval { start: s, end: e.min(total) });
            }
        }
    }
    if let Some(s) = open {
        // Silence running until EOF.
        out.push(Interval { start: s, end: total });
    }
    out
}

/// Complement of silences, each padded by `pad` seconds and merged.
pub fn keep_intervals(silences: &[Interval], total: f64, pad: f64) -> Vec<Interval> {
    let mut keeps = Vec::new();
    let mut cursor = 0.0;
    for s in silences {
        if s.start > cursor {
            keeps.push(Interval { start: cursor, end: s.start });
        }
        cursor = cursor.max(s.end);
    }
    if cursor < total {
        keeps.push(Interval { start: cursor, end: total });
    }
    let mut merged: Vec<Interval> = Vec::new();
    for k in keeps {
        let k = Interval { start: (k.start - pad).max(0.0), end: (k.end + pad).min(total) };
        if k.end - k.start < 0.05 {
            continue;
        }
        match merged.last_mut() {
            Some(last) if k.start <= last.end => last.end = last.end.max(k.end),
            _ => merged.push(k),
        }
    }
    merged
}

/// Split `[0, total)` into chunks no longer than `max_len`, cutting at the latest
/// available cut point within each window. Falls back to a hard cut when a window
/// has no cut point in its second half.
pub fn plan_chunks(total: f64, cut_points: &[f64], max_len: f64) -> Vec<Interval> {
    let mut chunks = Vec::new();
    let mut start = 0.0;
    while total - start > max_len {
        let limit = start + max_len;
        let cut = cut_points
            .iter()
            .copied()
            .filter(|&c| c > start + max_len * 0.5 && c <= limit)
            .fold(None, |acc: Option<f64>, c| Some(acc.map_or(c, |a| a.max(c))))
            .unwrap_or(limit);
        chunks.push(Interval { start, end: cut });
        start = cut;
    }
    if total - start > 0.01 || chunks.is_empty() {
        chunks.push(Interval { start, end: total });
    }
    chunks
}

// ---------------------------------------------------------------------------
// Voice activity detection
// ---------------------------------------------------------------------------

const SAMPLE_RATE: f64 = 16_000.0;
/// earshot works on 16 ms frames at 16 kHz.
const VAD_FRAME: usize = 256;

/// Open a 16-bit mono PCM WAV and position the reader at the sample data.
/// Returns the reader and the number of samples. Streams, so 6 h of audio doesn't need 700 MB of RAM.
fn open_wav(path: &Path) -> Result<(BufReader<File>, u64)> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut r = BufReader::with_capacity(1 << 20, file);
    let mut head = [0u8; 12];
    r.read_exact(&mut head)?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let mut pos = 12u64;
    loop {
        let mut ch = [0u8; 8];
        r.read_exact(&mut ch).context("WAV file has no data chunk")?;
        let size = u32::from_le_bytes(ch[4..8].try_into().unwrap()) as u64;
        pos += 8;
        if &ch[0..4] == b"data" {
            // ffmpeg may write 0xFFFFFFFF for streamed output; trust the file length instead.
            let bytes = size.min(file_len.saturating_sub(pos));
            return Ok((r, bytes / 2));
        }
        let skip = size + (size & 1);
        r.seek_relative(skip as i64)?;
        pos += skip;
    }
}

/// Call `f` with consecutive frames of `frame` samples (the last one may be shorter).
fn for_each_frame(path: &Path, frame: usize, mut f: impl FnMut(&[i16])) -> Result<u64> {
    let (mut r, total) = open_wav(path)?;
    let mut bytes = vec![0u8; frame * 2];
    let mut samples = vec![0i16; frame];
    let mut left = total;
    while left > 0 {
        let n = (left as usize).min(frame);
        r.read_exact(&mut bytes[..n * 2])?;
        for (i, b) in bytes[..n * 2].chunks_exact(2).enumerate() {
            samples[i] = i16::from_le_bytes([b[0], b[1]]);
        }
        f(&samples[..n]);
        left -= n as u64;
    }
    Ok(total)
}

fn wav_header(samples: u64) -> [u8; 44] {
    let data = (samples * 2).min(u32::MAX as u64 - 36) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
    h[24..28].copy_from_slice(&16_000u32.to_le_bytes());
    h[28..32].copy_from_slice(&32_000u32.to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

/// Copy only the kept sample ranges of a 16 kHz mono WAV into a new WAV (sample-exact).
pub fn condense_wav(input: &Path, output: &Path, keeps: &[Interval]) -> Result<()> {
    let ranges: Vec<(u64, u64)> =
        keeps.iter().map(|k| ((k.start * SAMPLE_RATE).round() as u64, (k.end * SAMPLE_RATE).round() as u64)).collect();
    let mut w = BufWriter::with_capacity(1 << 20, File::create(output)?);
    w.write_all(&wav_header(0))?;
    let (mut written, mut idx, mut ri) = (0u64, 0u64, 0usize);
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 16);
    for_each_frame(input, 1 << 15, |frame| {
        let (start, end) = (idx, idx + frame.len() as u64);
        buf.clear();
        while ri < ranges.len() && ranges[ri].0 < end {
            let (a, b) = ranges[ri];
            let (from, to) = (a.max(start), b.min(end));
            if from < to {
                for s in &frame[(from - start) as usize..(to - start) as usize] {
                    buf.extend_from_slice(&s.to_le_bytes());
                }
                written += to - from;
            }
            if b <= end {
                ri += 1;
            } else {
                break;
            }
        }
        let _ = w.write_all(&buf);
        idx = end;
    })?;
    w.flush()?;
    let mut f = w.into_inner().map_err(|e| anyhow!("{e}"))?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&wav_header(written))?;
    Ok(())
}

/// Stretches of speech according to the neural VAD, streamed from a 16 kHz mono WAV.
/// Runs shorter than 100 ms are ignored as clicks. Returns (speech, total seconds).
pub fn speech_intervals(path: &Path, threshold: f32) -> Result<(Vec<Interval>, f64)> {
    let mut detector = earshot::Detector::default();
    let frame_secs = VAD_FRAME as f64 / SAMPLE_RATE;
    let mut out = Vec::new();
    let mut start: Option<f64> = None;
    let mut frame = [0i16; VAD_FRAME];
    let mut i = 0usize;
    let total = for_each_frame(path, VAD_FRAME, |chunk| {
        frame[..chunk.len()].copy_from_slice(chunk);
        frame[chunk.len()..].fill(0);
        let t = i as f64 * frame_secs;
        let speech = detector.predict_i16(&frame) >= threshold;
        match (speech, start) {
            (true, None) => start = Some(t),
            (false, Some(s)) => {
                out.push(Interval { start: s, end: t });
                start = None;
            }
            _ => {}
        }
        i += 1;
    })? as f64
        / SAMPLE_RATE;
    if let Some(s) = start {
        out.push(Interval { start: s, end: total });
    }
    out.retain(|i| i.end - i.start >= 0.1);
    Ok((out, total))
}

/// Gaps between speech of at least `min_silence` seconds, in the same shape as `parse_silences`.
pub fn non_speech(speech: &[Interval], total: f64, min_silence: f64) -> Vec<Interval> {
    let mut out = Vec::new();
    let mut cursor = 0.0;
    for s in speech {
        if s.start - cursor >= min_silence {
            out.push(Interval { start: cursor, end: s.start });
        }
        cursor = cursor.max(s.end);
    }
    if total - cursor >= min_silence {
        out.push(Interval { start: cursor, end: total });
    }
    out
}

// ---------------------------------------------------------------------------
// ffmpeg invocation
// ---------------------------------------------------------------------------

/// Bundled sidecar sits next to the app executable; fall back to PATH.
pub fn ffmpeg_path() -> PathBuf {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let p = dir.join(name);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(name)
}

async fn ffmpeg(args: &[&str]) -> Result<String> {
    let out = Command::new(ffmpeg_path())
        .arg("-hide_banner")
        .arg("-nostdin")
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to run ffmpeg")?;
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        let tail: Vec<&str> = stderr.lines().rev().take(8).collect();
        bail!("ffmpeg failed: {}", tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    Ok(stderr)
}

/// `creation_time` metadata tag (set by phones, voice recorders, cameras).
pub fn parse_creation_time(stderr: &str) -> Option<String> {
    let re = Regex::new(r"(?m)^\s*creation_time\s*:\s*(\S+)").unwrap();
    let v = re.captures(stderr)?[1].to_string();
    // Ignore placeholder epochs some encoders write.
    (!v.starts_with("1970") && !v.starts_with("1904")).then_some(v)
}

fn parse_duration(stderr: &str) -> Option<f64> {
    let re = Regex::new(r"Duration:\s*(\d+):(\d+):([\d.]+)").unwrap();
    let c = re.captures(stderr)?;
    Some(c[1].parse::<f64>().ok()? * 3600.0 + c[2].parse::<f64>().ok()? * 60.0 + c[3].parse::<f64>().ok()?)
}

#[derive(Debug, Clone, Deserialize)]
pub struct PreprocessOptions {
    pub speed: f64,
    pub remove_silence: bool,
    pub silence_threshold_db: f64,
    pub silence_min_duration: f64,
    pub chunk_seconds: f64,
    pub method: SilenceMethod,
    pub vad_threshold: f64,
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub path: PathBuf,
    /// Processed-time offset of this chunk in the condensed audio.
    pub offset: f64,
    pub duration: f64,
}

pub struct Prepared {
    pub original_duration: f64,
    pub processed_duration: f64,
    pub map: TimeMap,
    pub chunks: Vec<Chunk>,
    pub recorded_at: Option<String>,
    /// The condensed (silence-removed, sped-up) WAV, kept so problem chunks can be re-cut.
    pub condensed: PathBuf,
}

/// Encode `[start, start + duration)` of the condensed WAV as a small Opus chunk.
pub async fn encode_chunk(condensed: &Path, start: f64, duration: f64, out: &Path) -> Result<Chunk> {
    let (ss, t) = (format!("{start:.3}"), format!("{duration:.3}"));
    let (src, dst) = (condensed.to_string_lossy().to_string(), out.to_string_lossy().to_string());
    ffmpeg(&[
        "-y", "-ss", &ss, "-t", &t, "-i", &src, "-ac", "1", "-c:a", "libopus", "-b:a", "24k", "-application", "voip", &dst,
    ])
    .await?;
    Ok(Chunk { path: out.to_path_buf(), offset: start, duration })
}

/// Cut `[start, start + duration)` of any audio file into a 16 kHz mono WAV.
/// The playback copy used when the webview can't play the original (macOS rejects long AAC
/// files with high sample rates: too many frames to index). Constant-bitrate MP3 needs no index
/// and seeks precisely.
pub fn playback_path(original: &Path) -> PathBuf {
    original.with_extension("play.mp3")
}

/// Make the playback copy (once). Returns its path.
pub async fn ensure_playback_copy(original: &Path) -> Result<PathBuf> {
    let out = playback_path(original);
    if out.exists() {
        return Ok(out);
    }
    let tmp = out.with_extension("part.mp3");
    let (src, dst) = (original.to_string_lossy().to_string(), tmp.to_string_lossy().to_string());
    ffmpeg(&["-y", "-i", &src, "-vn", "-ac", "1", "-ar", "24000", "-c:a", "libmp3lame", "-b:a", "48k", &dst]).await?;
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

/// Loudness over time for a waveform/level display: one value per `LEVEL_STEP` seconds,
/// 0..255 mapped from -60 dBFS..0 dBFS (RMS). Streams ffmpeg's PCM output, so long recordings
/// don't need a temp file.
pub const LEVEL_STEP: f64 = 0.1;

pub async fn loudness(input: &Path) -> Result<Vec<u8>> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    const RATE: usize = 4000;
    let window = (RATE as f64 * LEVEL_STEP) as usize;
    let src = input.to_string_lossy().to_string();
    let mut child = Command::new(ffmpeg_path())
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i", &src, "-vn", "-ac", "1", "-ar", "4000", "-f", "s16le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to run ffmpeg")?;
    let mut out = child.stdout.take().unwrap();
    let mut buf = vec![0u8; 1 << 16];
    let mut carry: Option<u8> = None;
    let (mut acc, mut n) = (0f64, 0usize);
    let mut levels = Vec::new();
    let push = |acc: f64, n: usize, levels: &mut Vec<u8>| {
        let rms = (acc / n.max(1) as f64).sqrt() / 32768.0;
        let db = 20.0 * rms.max(1e-9).log10();
        levels.push(((db + 60.0) / 60.0 * 255.0).clamp(0.0, 255.0) as u8);
    };
    loop {
        let r = out.read(&mut buf).await?;
        if r == 0 {
            break;
        }
        let mut bytes = &buf[..r];
        if let Some(lo) = carry.take() {
            let v = i16::from_le_bytes([lo, bytes[0]]) as f64;
            acc += v * v;
            n += 1;
            bytes = &bytes[1..];
        }
        for b in bytes.chunks_exact(2) {
            let v = i16::from_le_bytes([b[0], b[1]]) as f64;
            acc += v * v;
            n += 1;
            if n == window {
                push(acc, n, &mut levels);
                acc = 0.0;
                n = 0;
            }
        }
        if bytes.len() % 2 == 1 {
            carry = Some(bytes[bytes.len() - 1]);
        }
    }
    if n > 0 {
        push(acc, n, &mut levels);
    }
    let status = child.wait().await?;
    if !status.success() && levels.is_empty() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err).await;
        }
        bail!("ffmpeg failed: {}", err.trim());
    }
    Ok(levels)
}

/// Cached loudness for `original` (computed once, stored next to it).
pub async fn ensure_levels(original: &Path) -> Result<Vec<u8>> {
    let cache = original.with_extension("levels.bin");
    if let Ok(data) = std::fs::read(&cache) {
        if !data.is_empty() {
            return Ok(data);
        }
    }
    let levels = loudness(original).await?;
    let tmp = cache.with_extension("part");
    if std::fs::write(&tmp, &levels).is_ok() {
        let _ = std::fs::rename(&tmp, &cache);
    }
    Ok(levels)
}

/// Decode a whole file to 16 kHz mono PCM WAV (what the speaker models expect).
pub async fn decode_wav16k(input: &Path, out: &Path) -> Result<()> {
    let (src, dst) = (input.to_string_lossy().to_string(), out.to_string_lossy().to_string());
    ffmpeg(&["-y", "-i", &src, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", &dst]).await?;
    Ok(())
}

pub async fn extract_range(input: &Path, start: f64, duration: f64, out: &Path) -> Result<()> {
    let (ss, t) = (format!("{start:.3}"), format!("{duration:.3}"));
    let (src, dst) = (input.to_string_lossy().to_string(), out.to_string_lossy().to_string());
    ffmpeg(&["-y", "-ss", &ss, "-t", &t, "-i", &src, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", &dst]).await?;
    Ok(())
}

/// Seconds of speech (VAD) in `[start, start + duration)` of an audio file.
pub async fn speech_seconds(input: &Path, start: f64, duration: f64, scratch: &Path) -> Result<f64> {
    extract_range(input, start, duration, scratch).await?;
    let path = scratch.to_path_buf();
    let speech = tokio::task::spawn_blocking(move || speech_intervals(&path, 0.5)).await??.0;
    let _ = std::fs::remove_file(scratch);
    Ok(speech.iter().map(|i| i.end - i.start).sum())
}

/// Where to split `[start, end)` in two: the cut point nearest the middle, else the middle.
pub fn split_point(cut_points: &[f64], start: f64, end: f64) -> f64 {
    let mid = (start + end) / 2.0;
    let (lo, hi) = (start + (end - start) * 0.25, end - (end - start) * 0.25);
    cut_points
        .iter()
        .copied()
        .filter(|&c| c > lo && c < hi)
        .min_by(|a, b| (a - mid).abs().total_cmp(&(b - mid).abs()))
        .unwrap_or(mid)
}

/// Progress callback: (message, 0..1 within preprocessing).
pub type OnProgress<'a> = &'a (dyn Fn(&str, f64) + Send + Sync);

pub async fn preprocess(input: &Path, work: &Path, opts: &PreprocessOptions, progress: OnProgress<'_>) -> Result<Prepared> {
    std::fs::create_dir_all(work)?;
    let speed = opts.speed.clamp(0.5, 2.0);
    let norm = work.join("norm.wav");
    let norm_s = norm.to_string_lossy().to_string();
    let input_s = input.to_string_lossy().to_string();

    // 1. Normalize to 16 kHz mono PCM.
    progress("Converting audio", 0.0);
    let probe = ffmpeg(&["-y", "-i", &input_s, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", &norm_s]).await?;
    // Only look at the input section, not the output's metadata.
    let recorded_at = parse_creation_time(probe.split("Output #0").next().unwrap_or(&probe));

    progress(if opts.method == SilenceMethod::Speech { "Detecting speech" } else { "Detecting silence" }, 0.3);
    // 2. Find what to drop: non-speech (neural VAD) or quiet stretches (volume threshold).
    let (total, silences, pad) = match opts.method {
        SilenceMethod::Speech => {
            let (path, th, min) = (norm.clone(), opts.vad_threshold as f32, opts.silence_min_duration);
            tokio::task::spawn_blocking(move || -> Result<(f64, Vec<Interval>, f64)> {
                let (speech, total) = speech_intervals(&path, th)?;
                // Slightly larger padding: VAD edges can clip soft word onsets.
                Ok((total, non_speech(&speech, total, min), 0.25))
            })
            .await??
        }
        SilenceMethod::Volume => {
            let af = format!("silencedetect=noise={}dB:d={}", opts.silence_threshold_db, opts.silence_min_duration);
            let stderr = ffmpeg(&["-i", &norm_s, "-af", &af, "-f", "null", "-"]).await?;
            let total = parse_duration(&stderr).ok_or_else(|| anyhow!("could not read audio duration"))?;
            (total, parse_silences(&stderr, total), 0.15)
        }
    };

    let keeps = if opts.remove_silence {
        // Align to the 10 ms frames used below so the time map matches the selected samples exactly.
        let r = |v: f64| (v * 100.0).round() / 100.0;
        keep_intervals(&silences, total, pad)
            .into_iter()
            .map(|k| Interval { start: r(k.start), end: r(k.end).min(total) })
            .filter(|k| k.end > k.start)
            .fold(Vec::<Interval>::new(), |mut acc, k| {
                match acc.last_mut() {
                    Some(last) if k.start <= last.end => last.end = last.end.max(k.end),
                    _ => acc.push(k),
                }
                acc
            })
    } else {
        vec![Interval { start: 0.0, end: total }]
    };
    if keeps.is_empty() {
        bail!("No speech found in the audio. Try the volume-based silence method or a lower speech threshold.");
    }

    // 3. Condense: copy kept ranges in Rust (exact, streams), then speed up with ffmpeg.
    progress("Removing silence and speeding up", 0.55);
    let needs_trim = keeps.len() > 1 || keeps[0].start > 0.0 || keeps[0].end < total;
    let trimmed = if needs_trim {
        let out = work.join("trimmed.wav");
        let (src, dst, k) = (norm.clone(), out.clone(), keeps.clone());
        tokio::task::spawn_blocking(move || condense_wav(&src, &dst, &k)).await??;
        out
    } else {
        norm.clone()
    };
    let condensed = if (speed - 1.0).abs() > 1e-3 {
        let out = work.join("condensed.wav");
        let (in_s, out_s) = (trimmed.to_string_lossy().to_string(), out.to_string_lossy().to_string());
        ffmpeg(&["-y", "-i", &in_s, "-af", &format!("atempo={speed:.3}"), "-c:a", "pcm_s16le", &out_s]).await?;
        out
    } else {
        trimmed
    };
    // atempo's output length can differ very slightly from nominal; calibrate the time map to the
    // real sample counts so timestamps don't drift over multi-hour recordings.
    let effective_speed = if (speed - 1.0).abs() > 1e-3 {
        let kept: f64 = keeps.iter().map(|k| ((k.end - k.start) * SAMPLE_RATE).round()).sum();
        let actual = open_wav(&condensed)?.1 as f64;
        if actual > 0.0 { (kept / actual).clamp(speed * 0.98, speed * 1.02) } else { speed }
    } else {
        1.0
    };
    let map = TimeMap::new(&keeps, effective_speed);
    let processed_duration = map.processed_duration();

    // 4. Chunk at silence boundaries.
    let cut_points: Vec<f64> = if opts.remove_silence {
        map.boundaries()
    } else {
        silences.iter().map(|s| (s.start + s.end) / 2.0 / effective_speed).collect()
    };
    let plan = plan_chunks(processed_duration, &cut_points, opts.chunk_seconds.max(30.0));
    let mut chunks = Vec::new();
    for (i, c) in plan.iter().enumerate() {
        progress(&format!("Encoding chunk {}/{}", i + 1, plan.len()), 0.7 + 0.3 * i as f64 / plan.len() as f64);
        chunks.push(encode_chunk(&condensed, c.start, c.end - c.start, &work.join(format!("chunk_{i:03}.ogg"))).await?);
    }

    // The intermediate WAVs are ~115 MB per hour each. Keep only the condensed one (needed to re-cut
    // problem chunks); the job's work dir is removed when it finishes.
    for f in ["norm.wav", "trimmed.wav"] {
        let p = work.join(f);
        if p != condensed && !cfg!(test) {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(Prepared { original_duration: total, processed_duration, map, chunks, recorded_at, condensed })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iv(s: f64, e: f64) -> Interval {
        Interval { start: s, end: e }
    }

    #[test]
    fn parses_silencedetect() {
        let log = "[silencedetect @ 0x1] silence_start: -0.002\n\
                   [silencedetect @ 0x1] silence_end: 1.5 | silence_duration: 1.5\n\
                   [silencedetect @ 0x1] silence_start: 10.25\n\
                   [silencedetect @ 0x1] silence_end: 12.75 | silence_duration: 2.5\n\
                   [silencedetect @ 0x1] silence_start: 19.0\n";
        assert_eq!(parse_silences(log, 20.0), vec![iv(0.0, 1.5), iv(10.25, 12.75), iv(19.0, 20.0)]);
    }

    #[test]
    fn parses_creation_time() {
        let log = "Input #0, mov,mp4,m4a, from 'memo.m4a':\n  Metadata:\n    major_brand     : M4A\n    creation_time   : 2026-09-30T02:15:04.000000Z\n";
        assert_eq!(parse_creation_time(log).as_deref(), Some("2026-09-30T02:15:04.000000Z"));
        assert_eq!(parse_creation_time("    creation_time   : 1970-01-01T00:00:00.000000Z"), None);
        assert_eq!(parse_creation_time("Duration: 00:00:01.00"), None);
    }

    #[test]
    fn keeps_are_padded_and_merged() {
        let k = keep_intervals(&[iv(0.0, 1.5), iv(10.0, 10.2), iv(19.0, 20.0)], 20.0, 0.15);
        // The 0.2s silence disappears after padding; leading/trailing silence trimmed.
        assert_eq!(k.len(), 1);
        assert!((k[0].start - 1.35).abs() < 1e-9 && (k[0].end - 19.15).abs() < 1e-9);
    }

    #[test]
    fn time_map_roundtrip() {
        // Keep [2,6) and [10,14) at 2x speed → processed [0,2) and [2,4).
        let m = TimeMap::new(&[iv(2.0, 6.0), iv(10.0, 14.0)], 2.0);
        assert!((m.processed_duration() - 4.0).abs() < 1e-9);
        assert!((m.to_original(0.0) - 2.0).abs() < 1e-9);
        assert!((m.to_original(1.0) - 4.0).abs() < 1e-9);
        assert!((m.to_original(2.0) - 10.0).abs() < 1e-9);
        assert!((m.to_original_end(2.0) - 6.0).abs() < 1e-9);
        assert!((m.to_original(3.5) - 13.0).abs() < 1e-9);
        assert!((m.to_original(99.0) - 14.0).abs() < 1e-9);
        assert_eq!(m.boundaries(), vec![2.0]);
    }

    #[test]
    fn chunks_cut_at_boundaries() {
        let c = plan_chunks(25.0, &[4.0, 8.0, 9.5, 17.0], 10.0);
        assert_eq!(c, vec![iv(0.0, 9.5), iv(9.5, 17.0), iv(17.0, 25.0)]);
        // No usable cut point → hard cut.
        let c = plan_chunks(25.0, &[], 10.0);
        assert_eq!(c, vec![iv(0.0, 10.0), iv(10.0, 20.0), iv(20.0, 25.0)]);
    }

    /// End-to-end: tones at known times survive silence removal + speedup and map back correctly.
    #[tokio::test]
    async fn pipeline_maps_back_to_original_time() {
        let dir = std::env::temp_dir().join(format!("transcriber-audio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.wav");
        let tones = [(2.0, 4.0), (10.0, 11.5), (20.0, 23.0)];
        let cond = tones.iter().map(|(a, b)| format!("between(t,{a},{b})")).collect::<Vec<_>>().join("+");
        let expr = format!("aevalsrc='if({cond},0.5*sin(2*PI*440*t),0)':s=16000:d=25");
        ffmpeg(&["-y", "-f", "lavfi", "-i", &expr, src.to_str().unwrap()]).await.unwrap();

        let opts = PreprocessOptions {
            speed: 1.25,
            remove_silence: true,
            silence_threshold_db: -35.0,
            silence_min_duration: 0.7,
            chunk_seconds: 30.0,
            method: SilenceMethod::Volume,
            vad_threshold: 0.5,
        };
        let p = preprocess(&src, &dir.join("work"), &opts, &|_, _| {}).await.unwrap();
        assert!((p.original_duration - 25.0).abs() < 0.05);
        // 6.5s of tone + padding, at 1.25x.
        assert!(p.processed_duration < 7.0, "processed {}", p.processed_duration);

        // Find tone onsets in the condensed audio and map them back.
        let condensed = dir.join("work").join("condensed.wav");
        let log = ffmpeg(&["-i", condensed.to_str().unwrap(), "-af", "silencedetect=noise=-40dB:d=0.05", "-f", "null", "-"])
            .await
            .unwrap();
        let ends: Vec<f64> = parse_silences(&log, p.processed_duration).iter().map(|s| s.end).filter(|&e| e < p.processed_duration - 0.05).collect();
        let onsets: Vec<f64> = ends.iter().map(|&t| p.map.to_original(t)).collect();
        assert_eq!(onsets.len(), 3, "onsets {onsets:?}");
        for (o, (a, _)) in onsets.iter().zip(tones) {
            assert!((o - a).abs() < 0.06, "mapped {o} expected {a}");
        }
        assert_eq!(p.chunks.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Loudness buckets follow the signal: silence at the floor, the tone well above it.
    #[tokio::test]
    async fn loudness_follows_signal() {
        let dir = std::env::temp_dir().join(format!("transcriber-levels-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.wav");
        let expr = "aevalsrc='if(between(t,2,4),0.5*sin(2*PI*440*t),0)':s=16000:d=6";
        ffmpeg(&["-y", "-f", "lavfi", "-i", expr, src.to_str().unwrap()]).await.unwrap();
        let lv = ensure_levels(&src).await.unwrap();
        let per_s = (1.0 / LEVEL_STEP) as usize;
        assert!((lv.len() as i64 - 6 * per_s as i64).abs() <= 1, "len {}", lv.len());
        assert!(lv[per_s] < 10, "silence {}", lv[per_s]);
        assert!(lv[3 * per_s] > 200, "tone {}", lv[3 * per_s]); // 0.5 amplitude sine ≈ -9 dBFS
        assert!(src.with_extension("levels.bin").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Speech separated by gaps of loud background noise: the volume method keeps the noise,
    /// the VAD drops it. Needs macOS `say` for real speech; skipped elsewhere.
    #[tokio::test]
    async fn vad_drops_noise_that_volume_keeps() {
        let dir = std::env::temp_dir().join(format!("transcriber-vad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let say = |text: &str, out: &Path| {
            std::process::Command::new("say").args(["-o", out.to_str().unwrap(), text]).status()
        };
        let (a, b) = (dir.join("a.aiff"), dir.join("b.aiff"));
        if say("Good morning everyone, let's review the quarterly revenue numbers.", &a).is_err() {
            eprintln!("skipping: `say` not available");
            return;
        }
        say("Thanks. Revenue grew twelve percent this quarter.", &b).unwrap();
        // 3s speech-free noise, speech A, 8s noise, speech B, 3s noise.
        let src = dir.join("noisy.wav");
        ffmpeg(&[
            "-y", "-f", "lavfi", "-t", "3", "-i", "anoisesrc=c=pink:a=0.05:r=16000",
            "-f", "lavfi", "-t", "8", "-i", "anoisesrc=c=pink:a=0.05:r=16000",
            "-f", "lavfi", "-t", "3", "-i", "anoisesrc=c=pink:a=0.05:r=16000",
            "-i", a.to_str().unwrap(), "-i", b.to_str().unwrap(),
            "-filter_complex", "[3]aresample=16000,aformat=channel_layouts=mono[a];[4]aresample=16000,aformat=channel_layouts=mono[b];[0][a][1][b][2]concat=n=5:v=0:a=1",
            "-ar", "16000", "-ac", "1", src.to_str().unwrap(),
        ])
        .await
        .unwrap();

        let run = |method| {
            let src = src.clone();
            let work = dir.join(format!("{method:?}"));
            async move {
                let opts = PreprocessOptions {
                    speed: 1.0, remove_silence: true, silence_threshold_db: -35.0, silence_min_duration: 0.7,
                    chunk_seconds: 600.0, method, vad_threshold: 0.5,
                };
                preprocess(&src, &work, &opts, &|_, _| {}).await.unwrap()
            }
        };
        let volume = run(SilenceMethod::Volume).await;
        let speech = run(SilenceMethod::Speech).await;
        println!("original {:.1}s · volume keeps {:.1}s · speech keeps {:.1}s",
            speech.original_duration, volume.processed_duration, speech.processed_duration);
        // Noise is louder than -35 dB, so the volume method removes (almost) nothing.
        assert!(volume.processed_duration > speech.original_duration * 0.9);
        // VAD keeps roughly just the two utterances (+ padding), dropping ~14s of noise.
        assert!(speech.processed_duration < speech.original_duration - 10.0);
        // The first utterance starts at 3s in the original.
        let first = speech.map.pieces[0].original_start;
        assert!((first - 3.0).abs() < 0.5, "first speech kept from {first}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Timing check for long recordings: `BENCH_AUDIO=/path/long.mp3 cargo test bench_preprocess -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn bench_preprocess() {
        let input = PathBuf::from(std::env::var("BENCH_AUDIO").unwrap());
        for method in [SilenceMethod::Speech, SilenceMethod::Volume] {
            let work = std::env::temp_dir().join(format!("transcriber-bench-{}", uuid::Uuid::new_v4()));
            let opts = PreprocessOptions {
                speed: 1.5, remove_silence: true, silence_threshold_db: -35.0, silence_min_duration: 0.7,
                chunk_seconds: 600.0, method: method.clone(), vad_threshold: 0.5,
            };
            let t = std::time::Instant::now();
            let p = preprocess(&input, &work, &opts, &|_, _| {}).await.unwrap();
            println!(
                "{method:?}: {:.1}s → original {:.0}s, sent {:.0}s, {} pieces, {} chunks",
                t.elapsed().as_secs_f64(), p.original_duration, p.processed_duration, p.map.pieces.len(), p.chunks.len()
            );
            let _ = std::fs::remove_dir_all(work);
        }
    }
}

