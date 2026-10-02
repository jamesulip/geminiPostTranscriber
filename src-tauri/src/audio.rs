//! Audio preprocessing: normalize, detect/remove silence, speed up, chunk.
//! Keeps a time map so transcript timestamps can be mapped back to the original file.

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;

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
}

pub async fn preprocess(input: &Path, work: &Path, opts: &PreprocessOptions) -> Result<Prepared> {
    std::fs::create_dir_all(work)?;
    let speed = opts.speed.clamp(0.5, 2.0);
    let norm = work.join("norm.wav");
    let norm_s = norm.to_string_lossy().to_string();
    let input_s = input.to_string_lossy().to_string();

    // 1. Normalize to 16 kHz mono PCM.
    let probe = ffmpeg(&["-y", "-i", &input_s, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", &norm_s]).await?;
    // Only look at the input section, not the output's metadata.
    let recorded_at = parse_creation_time(probe.split("Output #0").next().unwrap_or(&probe));

    // 2. Silence detection (also gives exact duration of normalized audio).
    let af = format!("silencedetect=noise={}dB:d={}", opts.silence_threshold_db, opts.silence_min_duration);
    let stderr = ffmpeg(&["-i", &norm_s, "-af", &af, "-f", "null", "-"]).await?;
    let total = parse_duration(&stderr).ok_or_else(|| anyhow!("could not read audio duration"))?;
    let silences = parse_silences(&stderr, total);

    let keeps = if opts.remove_silence {
        // Align to the 10 ms frames used below so the time map matches the selected samples exactly.
        let r = |v: f64| (v * 100.0).round() / 100.0;
        keep_intervals(&silences, total, 0.15)
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
        bail!("audio appears to be entirely silent");
    }
    let map = TimeMap::new(&keeps, speed);

    // 3. Condense: select kept ranges (10 ms frames for precision) and apply tempo.
    let needs_trim = keeps.len() > 1 || keeps[0].start > 0.0 || keeps[0].end < total;
    let condensed = if needs_trim || (speed - 1.0).abs() > 1e-3 {
        let mut filters = vec!["asetnsamples=n=160".to_string()];
        if needs_trim {
            let expr = keeps
                .iter()
                .map(|k| format!("gte(t,{:.4})*lt(t,{:.4})", k.start - 0.001, k.end - 0.001))
                .collect::<Vec<_>>()
                .join("+");
            filters.push(format!("aselect='{expr}'"));
            filters.push("asetpts=N/SR/TB".into());
        }
        if (speed - 1.0).abs() > 1e-3 {
            filters.push(format!("atempo={speed:.3}"));
        }
        let script = work.join("filter.txt");
        std::fs::write(&script, filters.join(","))?;
        let out = work.join("condensed.wav");
        let out_s = out.to_string_lossy().to_string();
        let script_s = script.to_string_lossy().to_string();
        ffmpeg(&["-y", "-i", &norm_s, "-/af", &script_s, "-c:a", "pcm_s16le", &out_s]).await?;
        out
    } else {
        norm.clone()
    };
    let processed_duration = map.processed_duration();

    // 4. Chunk at silence boundaries.
    let cut_points: Vec<f64> = if opts.remove_silence {
        map.boundaries()
    } else {
        silences.iter().map(|s| (s.start + s.end) / 2.0 / speed).collect()
    };
    let plan = plan_chunks(processed_duration, &cut_points, opts.chunk_seconds.max(30.0));
    let condensed_s = condensed.to_string_lossy().to_string();
    let mut chunks = Vec::new();
    for (i, c) in plan.iter().enumerate() {
        let path = work.join(format!("chunk_{i:03}.ogg"));
        let path_s = path.to_string_lossy().to_string();
        let ss = format!("{:.3}", c.start);
        let t = format!("{:.3}", c.end - c.start);
        ffmpeg(&[
            "-y", "-ss", &ss, "-t", &t, "-i", &condensed_s, "-ac", "1", "-c:a", "libopus", "-b:a", "24k",
            "-application", "voip", &path_s,
        ])
        .await?;
        chunks.push(Chunk { path, offset: c.start, duration: c.end - c.start });
    }

    Ok(Prepared { original_duration: total, processed_duration, map, chunks, recorded_at })
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
        };
        let p = preprocess(&src, &dir.join("work"), &opts).await.unwrap();
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
}

