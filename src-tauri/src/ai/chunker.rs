//! Splits a transcript into retrieval chunks of a few minutes, cut where the conversation
//! naturally breaks (speaker change or pause) once the chunk is big enough.

use super::{estimate_tokens, format_time};
use crate::db::{content_hash, SegmentRow};
use std::collections::HashMap;

pub const TARGET_TOKENS: usize = 2000;
pub const MAX_TOKENS: usize = 3500;
/// A pause this long counts as a natural break.
const GAP_SECS: f64 = 2.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub idx: i64,
    pub start: f64,
    pub end: f64,
    pub first_idx: i64,
    pub last_idx: i64,
    /// Hash of the segments' timing/labels/text (not display names).
    pub hash: String,
    /// What the model sees: one `[time] Name: text` line per segment.
    pub text: String,
}

/// "[12:05] Maria: text" — display names are resolved at render time so renames need no re-embedding.
pub fn render_line(s: &SegmentRow, names: &HashMap<String, String>) -> String {
    let who = s.speaker.as_deref().map(|l| names.get(l).map(String::as_str).unwrap_or(l));
    match who {
        Some(w) => format!("[{}] {w}: {}", format_time(s.start), s.text.trim()),
        None => format!("[{}] {}", format_time(s.start), s.text.trim()),
    }
}

pub fn render(segs: &[SegmentRow], names: &HashMap<String, String>) -> String {
    segs.iter().map(|s| render_line(s, names)).collect::<Vec<_>>().join("\n")
}

pub fn chunk(segs: &[SegmentRow], names: &HashMap<String, String>) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut from = 0usize;
    let mut tokens = 0usize;
    for i in 0..segs.len() {
        tokens += estimate_tokens(&segs[i].text) + 6;
        let last = i + 1 == segs.len();
        let natural = !last && {
            let (a, b) = (&segs[i], &segs[i + 1]);
            a.speaker != b.speaker || b.start - a.end >= GAP_SECS
        };
        if last || (tokens >= TARGET_TOKENS && natural) || tokens >= MAX_TOKENS {
            let part = &segs[from..=i];
            out.push(Chunk {
                idx: out.len() as i64,
                start: part[0].start,
                end: part.iter().map(|s| s.end).fold(0.0, f64::max),
                first_idx: part[0].idx,
                last_idx: part[part.len() - 1].idx,
                hash: content_hash(part),
                text: render(part, names),
            });
            from = i + 1;
            tokens = 0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(idx: i64, start: f64, speaker: &str, words: usize) -> SegmentRow {
        SegmentRow { id: idx, idx, start, end: start + 10.0, speaker: Some(speaker.into()), text: "salita ".repeat(words) }
    }

    #[test]
    fn cuts_at_speaker_changes_once_big_enough() {
        // 40 segments × ~230 tokens; speaker changes every 5 segments.
        let segs: Vec<SegmentRow> = (0..40).map(|i| seg(i, i as f64 * 10.0, if (i / 5) % 2 == 0 { "A" } else { "B" }, 100)).collect();
        let chunks = chunk(&segs, &HashMap::new());
        assert!(chunks.len() >= 3);
        for c in &chunks {
            assert!(estimate_tokens(&c.text) <= MAX_TOKENS + 200, "chunk too big");
        }
        // Boundaries fall on speaker changes (every 5th segment) unless the cap forced a cut.
        assert!(chunks.iter().all(|c| (c.last_idx + 1) % 5 == 0 || c.last_idx == 39));
        assert_eq!(chunks.last().unwrap().last_idx, 39);
        // Same text → same hash; a text edit changes only its chunk's hash.
        let mut edited = segs.clone();
        edited[1].text.push_str(" iba");
        let again = chunk(&edited, &HashMap::new());
        assert_ne!(again[0].hash, chunks[0].hash);
        assert_eq!(again[1].hash, chunks[1].hash);
        // Display names render but don't affect the hash.
        let names = HashMap::from([("A".to_string(), "Maria".to_string())]);
        let named = chunk(&segs, &names);
        assert!(named[0].text.starts_with("[0:00] Maria:"));
        assert_eq!(named[0].hash, chunks[0].hash);
    }
}
