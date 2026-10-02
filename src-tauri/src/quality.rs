//! Sanity checks for a chunk's transcript. Speech models (especially on long audio) sometimes
//! skip minutes of speech, loop on the same lines, or return impossible timings.

use crate::providers::Segment;

/// With silence removed, the audio sent is nearly all speech; this long without text means
/// something was skipped.
const HOLE_SECS_CONDENSED: f64 = 45.0;
/// Without silence removal, real pauses can be long.
const HOLE_SECS_RAW: f64 = 180.0;
/// No single segment should last this long.
const MAX_SEGMENT_SECS: f64 = 120.0;
/// Chunks shorter than this aren't checked (too little to judge).
const MIN_CHECK_SECS: f64 = 60.0;

/// Lowercase letters/digits only, for comparing lines.
fn normalize(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Remove lines that repeat one of the few lines just before them (a model looping), and exact
/// duplicates (same text at the same time, however short). Short utterances said again later
/// ("okay", "yes") are kept.
pub fn remove_loops(segs: Vec<Segment>) -> (Vec<Segment>, usize) {
    const WINDOW: usize = 8;
    let mut out: Vec<Segment> = Vec::with_capacity(segs.len());
    let mut recent: Vec<(String, f64, f64)> = Vec::new();
    let mut removed = 0;
    for s in segs {
        let key = normalize(&s.text);
        let repeat = recent.iter().any(|(k, start, end)| {
            k == &key && (key.chars().count() >= 8 || ((start - s.start).abs() < 0.05 && (end - s.end).abs() < 0.05))
        });
        if repeat {
            removed += 1;
            continue;
        }
        recent.push((key, s.start, s.end));
        if recent.len() > WINDOW {
            recent.remove(0);
        }
        out.push(s);
    }
    (out, removed)
}

pub struct CheckOptions {
    /// Audio had silence removed (so long text gaps are suspicious).
    pub condensed: bool,
    /// Speaker labels were requested.
    pub want_speakers: bool,
}

/// Describe the first problem found in a chunk's segments (chunk-relative times), if any.
pub fn check(segs: &[Segment], duration: f64, o: &CheckOptions) -> Option<String> {
    if duration < MIN_CHECK_SECS {
        return None;
    }
    if segs.is_empty() {
        return Some("no text returned".into());
    }
    let mut sorted: Vec<&Segment> = segs.iter().collect();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));

    if let Some(s) = sorted.iter().find(|s| s.end - s.start > MAX_SEGMENT_SECS) {
        return Some(format!("a single line spans {:.0}s at {}", s.end - s.start, clock(s.start)));
    }

    let hole = if o.condensed { HOLE_SECS_CONDENSED } else { HOLE_SECS_RAW };
    let mut cursor = 0.0;
    for s in &sorted {
        if s.start - cursor > hole {
            return Some(format!("no text for {:.0}s at {}", s.start - cursor, clock(cursor)));
        }
        cursor = f64::max(cursor, s.end);
    }
    if duration - cursor > hole {
        return Some(format!("no text for the last {:.0}s", duration - cursor));
    }

    let (_, looped) = remove_loops(sorted.iter().map(|s| (*s).clone()).collect());
    if looped >= 5 && looped * 10 >= segs.len() {
        return Some(format!("{looped} repeated lines (model looping)"));
    }

    if o.want_speakers && segs.len() >= 8 {
        let labeled = segs.iter().filter(|s| s.speaker.is_some()).count();
        if labeled * 5 < segs.len() {
            return Some("speaker labels missing".into());
        }
    }
    None
}

fn clock(t: f64) -> String {
    let s = t.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, text: &str) -> Segment {
        Segment { start, end, text: text.into(), speaker: Some("Speaker 1".into()) }
    }

    /// One line every 5 s across the chunk.
    fn steady(duration: f64) -> Vec<Segment> {
        (0..(duration / 5.0) as usize).map(|i| seg(i as f64 * 5.0, i as f64 * 5.0 + 4.0, &format!("line number {i}"))).collect()
    }

    const O: CheckOptions = CheckOptions { condensed: true, want_speakers: true };

    #[test]
    fn healthy_chunk_passes() {
        assert_eq!(check(&steady(600.0), 600.0, &O), None);
        assert_eq!(check(&[], 30.0, &O), None, "short chunks are not judged");
    }

    #[test]
    fn detects_holes() {
        let mut s = steady(600.0);
        s.retain(|x| x.start < 200.0 || x.start > 400.0);
        assert!(check(&s, 600.0, &O).unwrap().starts_with("no text for"));
        let s: Vec<_> = steady(600.0).into_iter().filter(|x| x.start < 300.0).collect();
        assert!(check(&s, 600.0, &O).unwrap().contains("last"));
        // Without silence removal, a 3-minute pause can be real.
        let mut s = steady(600.0);
        s.retain(|x| x.start < 200.0 || x.start > 330.0);
        assert_eq!(check(&s, 600.0, &CheckOptions { condensed: false, want_speakers: true }), None);
    }

    #[test]
    fn detects_giant_segments_loops_and_missing_speakers() {
        let mut s = steady(600.0);
        s.push(seg(10.0, 500.0, "everything at once"));
        assert!(check(&s, 600.0, &O).unwrap().contains("single line"));

        let mut s = steady(600.0);
        for i in 0..20 {
            s.insert(10, seg(50.0 + i as f64, 51.0 + i as f64, if i % 2 == 0 { "Exactly the workflow" } else { "Ah this is the number" }));
        }
        assert!(check(&s, 600.0, &O).unwrap().contains("repeated"));

        let s: Vec<_> = steady(600.0).into_iter().map(|mut x| { x.speaker = None; x }).collect();
        assert_eq!(check(&s, 600.0, &O).as_deref(), Some("speaker labels missing"));
        assert_eq!(check(&s, 600.0, &CheckOptions { condensed: true, want_speakers: false }), None);
    }

    #[test]
    fn removes_loops_but_keeps_short_repeats() {
        let s = vec![seg(0.0, 1.0, "Okay."), seg(1.0, 2.0, "Okay."), seg(2.0, 3.0, "Tapos 'yung workflow"),
            seg(3.0, 4.0, "Hindi ganun"), seg(4.0, 5.0, "Tapos 'yung workflow")];
        let (out, removed) = remove_loops(s);
        assert_eq!(removed, 1);
        assert_eq!(out.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["Okay.", "Okay.", "Tapos 'yung workflow", "Hindi ganun"]);

        // Short line duplicated at the exact same time (as in Sep 24): removed.
        let s = vec![seg(5543.8, 5545.3, "Tapos 'O"), seg(5543.8, 5545.3, "Tapos 'O"), seg(5543.8, 5545.3, "Tapos 'O")];
        assert_eq!(remove_loops(s).1, 2);
    }
}
