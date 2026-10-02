//! Find bad stretches in an existing transcript so only those can be re-transcribed.

use crate::audio;
use crate::db::SegmentRow;
use crate::quality;
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

/// Gaps without text longer than this are checked for speech.
const GAP_SECS: f64 = 60.0;
/// Problem stretches closer than this are merged into one.
const MERGE_SECS: f64 = 90.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProblemRange {
    pub start: f64,
    pub end: f64,
    /// Human-readable reasons, e.g. "no text for 12:45 but speech detected".
    pub reasons: Vec<String>,
    /// "text" (missing/garbled text) or "speakers" (only speaker labels missing).
    pub kind: String,
}

fn span(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    if m >= 1 { format!("{m} min") } else { format!("{secs:.0} s") }
}

/// Problems that don't need the audio: impossible timings, loops, missing speaker labels.
pub fn text_problems(segs: &[SegmentRow], want_speakers: bool) -> Vec<ProblemRange> {
    let mut out = Vec::new();
    for s in segs.iter().filter(|s| s.end - s.start > 120.0) {
        out.push(ProblemRange {
            start: s.start,
            end: s.start + 120.0,
            reasons: vec![format!("one line spans {} (impossible timing)", span(s.end - s.start))],
            kind: "text".into(),
        });
    }

    // Repeated lines: find which segments `remove_loops` would drop, and cluster them in time.
    let as_segments: Vec<crate::providers::Segment> = segs
        .iter()
        .map(|s| crate::providers::Segment { start: s.start, end: s.end, text: s.text.clone(), speaker: s.speaker.clone() })
        .collect();
    let (kept, _) = quality::remove_loops(as_segments.clone());
    let mut kept_iter = kept.iter().peekable();
    let mut dup_times: Vec<f64> = Vec::new();
    for s in &as_segments {
        match kept_iter.peek() {
            Some(k) if k.start == s.start && k.text == s.text && k.end == s.end => {
                kept_iter.next();
            }
            _ => dup_times.push(s.start),
        }
    }
    let mut i = 0;
    while i < dup_times.len() {
        let mut j = i;
        while j + 1 < dup_times.len() && dup_times[j + 1] - dup_times[j] < 120.0 {
            j += 1;
        }
        let n = j - i + 1;
        if n >= 3 {
            out.push(ProblemRange {
                start: dup_times[i] - 30.0,
                end: dup_times[j] + 30.0,
                reasons: vec![format!("{n} repeated lines")],
                kind: "text".into(),
            });
        }
        i = j + 1;
    }

    if want_speakers {
        let mut run_start: Option<f64> = None;
        let mut last_end = 0.0;
        let flush = |from: Option<f64>, to: f64, out: &mut Vec<ProblemRange>| {
            if let Some(a) = from {
                if to - a >= 180.0 {
                    out.push(ProblemRange {
                        start: a,
                        end: to,
                        reasons: vec![format!("no speaker labels for {}", span(to - a))],
                        kind: "speakers".into(),
                    });
                }
            }
        };
        for s in segs {
            if s.speaker.is_none() {
                run_start.get_or_insert(s.start);
                last_end = s.end;
            } else {
                flush(run_start.take(), last_end, &mut out);
            }
        }
        flush(run_start, last_end, &mut out);
    }
    out
}

/// Gaps without text that actually contain speech (checked with the VAD on the original audio).
pub async fn missing_speech(segs: &[SegmentRow], duration: f64, audio_path: &Path, scratch: &Path) -> Result<Vec<ProblemRange>> {
    let mut sorted: Vec<&SegmentRow> = segs.iter().filter(|s| s.end - s.start <= 120.0).collect();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut gaps = Vec::new();
    let mut cursor = 0.0;
    for s in &sorted {
        if s.start - cursor > GAP_SECS {
            gaps.push((cursor, s.start));
        }
        cursor = f64::max(cursor, s.end);
    }
    if duration - cursor > GAP_SECS {
        gaps.push((cursor, duration));
    }
    let mut out = Vec::new();
    for (a, b) in gaps {
        let speech = audio::speech_seconds(audio_path, a, b - a, scratch).await?;
        if speech >= f64::max(20.0, 0.25 * (b - a)) {
            out.push(ProblemRange {
                start: a,
                end: b,
                reasons: vec![format!("no text for {}, but {} of speech in the audio", span(b - a), span(speech))],
                kind: "text".into(),
            });
        }
    }
    Ok(out)
}

/// Sort, pad, clamp and merge nearby ranges.
pub fn merge_ranges(mut ranges: Vec<ProblemRange>, duration: f64) -> Vec<ProblemRange> {
    for r in &mut ranges {
        r.start = (r.start - 5.0).max(0.0).floor();
        r.end = (r.end + 5.0).min(duration).ceil();
    }
    ranges.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<ProblemRange> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            // Text and speaker-only problems stay separate so the user can choose which to fix.
            Some(last) if r.start - last.end < MERGE_SECS && last.kind == r.kind => {
                last.end = last.end.max(r.end);
                for reason in r.reasons {
                    if !last.reasons.contains(&reason) {
                        last.reasons.push(reason);
                    }
                }
            }
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(i: i64, start: f64, end: f64, speaker: Option<&str>, text: &str) -> SegmentRow {
        SegmentRow { id: i, idx: i, start, end, speaker: speaker.map(str::to_string), text: text.into() }
    }

    #[test]
    fn finds_loops_giant_lines_and_missing_speakers() {
        let mut segs: Vec<SegmentRow> = (0..100).map(|i| row(i, i as f64 * 5.0, i as f64 * 5.0 + 4.0, Some("Speaker 1"), &format!("line {i} here"))).collect();
        // A loop at 300 s (identical lines, identical times), as in Sep 24.
        for k in 0..10 {
            segs.push(row(200 + k, 300.0, 301.5, None, "Ah ito 'yung number tapos sir"));
        }
        // A 29-minute "line".
        segs.push(row(300, 400.0, 2140.0, Some("Speaker 1"), "garbled"));
        segs.sort_by(|a, b| a.start.total_cmp(&b.start));
        let p = text_problems(&segs, false);
        assert!(p.iter().any(|r| r.reasons[0] == "9 repeated lines" && r.start <= 300.0 && r.end >= 301.0), "{p:?}");
        assert!(p.iter().any(|r| r.reasons[0].contains("impossible timing") && r.start == 400.0));

        let unlabeled: Vec<SegmentRow> = (0..100).map(|i| row(i, i as f64 * 5.0, i as f64 * 5.0 + 4.0, None, &format!("line {i}"))).collect();
        let p = text_problems(&unlabeled, true);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, "speakers");
    }

    #[test]
    fn merges_nearby_ranges() {
        let r = |a: f64, b: f64, k: &str| ProblemRange { start: a, end: b, reasons: vec![format!("{a}")], kind: k.into() };
        let m = merge_ranges(vec![r(1000.0, 1100.0, "speakers"), r(100.0, 200.0, "text"), r(250.0, 300.0, "text")], 1200.0);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].start, m[0].end, m[0].reasons.len()), (95.0, 305.0, 2));
        assert_eq!((m[1].start, m[1].end, m[1].kind.as_str()), (995.0, 1105.0, "speakers"));
    }

    /// Run detection on a real library copy: `REPAIR_DB=... REPAIR_ID=... cargo test detect_real -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn detect_real() {
        let db = crate::db::open(Path::new(&std::env::var("REPAIR_DB").unwrap())).unwrap();
        let t = crate::db::get_transcript(&db, &std::env::var("REPAIR_ID").unwrap()).unwrap().unwrap();
        let mut ranges = text_problems(&t.segments, true);
        let scratch = std::env::temp_dir().join("vad-detect-real.wav");
        ranges.extend(missing_speech(&t.segments, t.summary.duration, Path::new(&t.audio_path), &scratch).await.unwrap());
        let merged = merge_ranges(ranges, t.summary.duration);
        let clock = |t: f64| { let s = t as u64; format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) };
        let mut text_secs = 0.0;
        for r in &merged {
            if r.kind == "text" { text_secs += r.end - r.start; }
            println!("{} – {}  [{}]  {}", clock(r.start), clock(r.end), r.kind, r.reasons.join("; "));
        }
        println!("text problems total: {:.1} min of {:.1} min", text_secs / 60.0, t.summary.duration / 60.0);
    }
}

