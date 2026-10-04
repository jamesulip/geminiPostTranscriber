//! Re-time transcript lines against word timings from a speech recognizer (Whisper).
//!
//! General models like Gemini Flash write timestamps themselves, and they drift by up to a minute
//! inside a part. The text is good, so it's kept: each line is moved to where its words are
//! actually spoken. Lines and reference words are matched as a weighted, order-preserving chain
//! (rare words count more), only between words within `WINDOW` seconds of each other. Lines with
//! too few matches get the shift of the matched lines around them.

use crate::db::SegmentRow;
use crate::providers::{Segment, Word};
use std::collections::HashMap;

/// How far a line's timestamp may be from where it's really spoken.
const WINDOW: f64 = 300.0;
/// Rough speaking time per word, for the unmatched words at the edges of a line.
const SECS_PER_WORD: f64 = 0.3;
/// A weakly matched line whose shift differs this much from the placed lines on both sides is a
/// stray match.
const STRAY_SECS: f64 = 15.0;

/// New times for one line (by segment id).
#[derive(Debug, Clone, PartialEq)]
pub struct LineTiming {
    pub id: i64,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug)]
pub struct Aligned {
    /// Every line, in the original order (starts never decrease).
    pub timings: Vec<LineTiming>,
    /// Lines placed by their own words (the rest were placed between them).
    pub matched: usize,
    /// Lines whose start moved by more than a second.
    pub moved: usize,
    pub max_shift: f64,
}

/// Lowercase words without punctuation, with common Taglish spelling variants made the same.
pub fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace(['\'', '’', '‘'], "")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            match w {
                "yon" | "iyon" => "yun",
                "yong" | "iyong" => "yung",
                "doon" | "don" => "dun",
                "diyan" | "jan" => "dyan",
                "niyo" => "nyo",
                "kase" => "kasi",
                w => w,
            }
            .to_string()
        })
        .collect()
}

/// The recognizer's words to align against, minus any it produced while looping (Whisper sometimes
/// repeats a line every second through long stretches): only words inside the segments that
/// survive `remove_loops` are kept. Without word timings, they're spread over the segments.
pub fn reference_words(segments: Vec<Segment>, words: Vec<Word>) -> Vec<Word> {
    let (kept, _) = crate::quality::remove_loops(segments);
    if words.is_empty() {
        return words_from_segments(&kept);
    }
    let mut spans: Vec<(f64, f64)> = kept.iter().map(|s| (s.start - 0.2, s.end + 0.2)).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    words
        .into_iter()
        .filter(|w| {
            let mid = (w.start + w.end) / 2.0;
            let k = spans.partition_point(|s| s.0 <= mid);
            spans[..k].iter().rev().take(4).any(|s| mid <= s.1)
        })
        .collect()
}

/// Word timings when the recognizer only gave segment times: spread each segment over its words
/// by character position.
pub fn words_from_segments(segs: &[Segment]) -> Vec<Word> {
    let mut out = Vec::new();
    for s in segs {
        let parts: Vec<&str> = s.text.split_whitespace().collect();
        let total: usize = parts.iter().map(|p| p.chars().count() + 1).sum();
        if total == 0 {
            continue;
        }
        let per_char = (s.end - s.start).max(0.0) / total as f64;
        let mut at = 0usize;
        for p in parts {
            let len = p.chars().count() + 1;
            out.push(Word { start: s.start + at as f64 * per_char, end: s.start + (at + len) as f64 * per_char, text: p.to_string() });
            at += len;
        }
    }
    out
}

/// Prefix maximum over reference positions, remembering which match gave it.
struct Fenwick(Vec<(f64, usize)>);

impl Fenwick {
    fn new(n: usize) -> Self {
        Fenwick(vec![(0.0, usize::MAX); n + 1])
    }
    /// Best chain ending at a reference position < `j`.
    fn best_before(&self, j: usize) -> (f64, usize) {
        let (mut i, mut best) = (j, (0.0, usize::MAX));
        while i > 0 {
            if self.0[i].0 > best.0 {
                best = self.0[i];
            }
            i &= i - 1;
        }
        best
    }
    fn update(&mut self, j: usize, v: (f64, usize)) {
        let mut i = j + 1;
        while i < self.0.len() {
            if v.0 > self.0[i].0 {
                self.0[i] = v;
            }
            i += i & i.wrapping_neg();
        }
    }
}

pub fn align(lines: &[SegmentRow], words: &[Word], duration: f64) -> Aligned {
    // Reference tokens in time order, each with its time span.
    let mut words: Vec<&Word> = words.iter().collect();
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut vocab: HashMap<String, u32> = HashMap::new();
    let (mut rtok, mut rt0, mut rt1) = (Vec::new(), Vec::new(), Vec::new());
    for w in words {
        let toks = tokens(&w.text);
        let step = (w.end - w.start).max(0.0) / toks.len().max(1) as f64;
        for (k, t) in toks.into_iter().enumerate() {
            let n = vocab.len() as u32;
            rtok.push(*vocab.entry(t).or_insert(n));
            rt0.push(w.start + k as f64 * step);
            rt1.push(w.start + (k + 1) as f64 * step);
        }
    }
    let mut postings: Vec<Vec<usize>> = vec![Vec::new(); vocab.len()];
    for (j, &t) in rtok.iter().enumerate() {
        postings[t as usize].push(j);
    }
    let total = rtok.len().max(1) as f64;
    let weight: Vec<f64> = postings.iter().map(|p| (1.0 + total / p.len() as f64).ln()).collect();

    // Candidate matches (line token → reference token) near the line's current time, as a chain
    // that keeps both orders. Matches of one line token are scored before any is stored, so a
    // chain uses each line token once.
    struct Match {
        line: usize,
        pos: usize,
        j: usize,
        score: f64,
        prev: usize,
    }
    let mut matches: Vec<Match> = Vec::new();
    let mut tree = Fenwick::new(rtok.len());
    let mut line_tokens: Vec<usize> = Vec::with_capacity(lines.len());
    for (li, l) in lines.iter().enumerate() {
        let toks = tokens(&l.text);
        line_tokens.push(toks.len());
        for (pos, t) in toks.iter().enumerate() {
            let Some(&tid) = vocab.get(t) else { continue };
            let at = l.start + (l.end - l.start).max(0.0) * (pos as f64 + 0.5) / toks.len() as f64;
            let p = &postings[tid as usize];
            let lo = p.partition_point(|&j| rt0[j] < at - WINDOW);
            let hi = p.partition_point(|&j| rt0[j] <= at + WINDOW);
            let first = matches.len();
            for &j in &p[lo..hi] {
                let (best, prev) = tree.best_before(j);
                matches.push(Match { line: li, pos, j, score: best + weight[tid as usize], prev });
            }
            for (m, x) in matches.iter().enumerate().skip(first) {
                tree.update(x.j, (x.score, m));
            }
        }
    }

    // Walk back the best chain: matched (token position, reference index) per line.
    let mut per_line: Vec<Vec<(usize, usize)>> = vec![Vec::new(); lines.len()];
    let mut at = tree.best_before(rtok.len()).1;
    while at != usize::MAX {
        let m = &matches[at];
        per_line[m.line].push((m.pos, m.j));
        at = m.prev;
    }

    // Lines with enough of their own words matched, in a plausible span, are placed by them.
    let mut placed: Vec<Option<(f64, f64)>> = vec![None; lines.len()];
    for (li, ms) in per_line.iter_mut().enumerate() {
        ms.reverse();
        let n = line_tokens[li];
        let needed = if n <= 2 { 1 } else { 2.max((n as f64 * 0.15).ceil() as usize) };
        let (Some(&(p0, j0)), Some(&(p1, j1))) = (ms.first(), ms.last()) else { continue };
        if ms.len() < needed || rt1[j1] - rt0[j0] > 3.0 + n as f64 {
            continue;
        }
        let start = rt0[j0] - p0 as f64 * SECS_PER_WORD;
        let end = rt1[j1] + (n - 1 - p1) as f64 * SECS_PER_WORD;
        placed[li] = Some((start.max(0.0), end));
    }

    // Drop stray placements: a few common words matched somewhere else, giving a shift unlike both
    // the lines before and after it. (Drift jumps at part boundaries, but a line there still agrees
    // with one side.) Lines matched by most of their own words are trusted: the model's error can
    // differ a lot from one line to the next.
    let strong = |li: usize| per_line[li].len() >= 4 && per_line[li].len() * 2 >= line_tokens[li];
    let shifts: Vec<(usize, f64)> =
        placed.iter().enumerate().filter_map(|(li, p)| p.map(|(s, _)| (li, s - lines[li].start))).collect();
    let median = |v: &[(usize, f64)]| -> Option<f64> {
        let mut x: Vec<f64> = v.iter().map(|a| a.1).collect();
        x.sort_by(f64::total_cmp);
        x.get(x.len() / 2).copied()
    };
    for (k, &(li, s)) in shifts.iter().enumerate() {
        let before = median(&shifts[k.saturating_sub(5)..k]);
        let after = median(&shifts[(k + 1).min(shifts.len())..(k + 6).min(shifts.len())]);
        if !strong(li)
            && [before, after].iter().all(|m| m.is_none_or(|m| (s - m).abs() > STRAY_SECS))
            && (before.is_some() || after.is_some())
        {
            placed[li] = None;
        }
    }

    // The rest go between the placed lines around them: by the shift of those lines, interpolated
    // over their old times, or (when the old times are unusable, e.g. several lines given the
    // same time) spread over the gap by text length.
    let mut times: Vec<(f64, f64)> = lines.iter().zip(&placed).map(|(l, p)| p.unwrap_or((l.start, l.end))).collect();
    let anchors: Vec<usize> = (0..lines.len()).filter(|&i| placed[i].is_some()).collect();
    let shift = |times: &[(f64, f64)], i: usize| times[i].0 - lines[i].start;
    let mut prev: Option<usize> = None;
    for k in 0..=anchors.len() {
        let next = anchors.get(k).copied();
        let run = prev.map_or(0, |p| p + 1)..next.unwrap_or(lines.len());
        let moved: Vec<(f64, f64)> = match (prev, next) {
            (None, None) => Vec::new(),
            (Some(a), None) | (None, Some(a)) => {
                let d = shift(&times, a);
                run.clone().map(|i| (lines[i].start + d, lines[i].end + d)).collect()
            }
            (Some(a), Some(b)) => {
                let (oa, ob) = (lines[a].start, lines[b].start);
                if ob - oa > 0.5 && run.clone().all(|i| lines[i].start > oa && lines[i].start < ob) {
                    let (da, db) = (shift(&times, a), shift(&times, b));
                    run.clone()
                        .map(|i| {
                            let d = da + (db - da) * (lines[i].start - oa) / (ob - oa);
                            (lines[i].start + d, lines[i].end + d)
                        })
                        .collect()
                } else {
                    let (lo, hi) = (times[a].1.max(times[a].0), times[b].0);
                    let span = (hi - lo).max(0.0);
                    let weight = |i: usize| lines[i].text.chars().count() as f64 + 10.0;
                    let total: f64 = run.clone().map(weight).sum();
                    let mut at = 0.0;
                    run.clone()
                        .map(|i| {
                            let start = lo + span * at / total;
                            at += weight(i);
                            (start, lo + span * at / total)
                        })
                        .collect()
                }
            }
        };
        for (i, t) in run.zip(moved) {
            times[i] = t;
        }
        prev = next;
    }

    let mut timings = Vec::with_capacity(lines.len());
    let mut last_start = 0.0f64;
    for (l, &(start, end)) in lines.iter().zip(&times) {
        let start = start.max(last_start).clamp(0.0, duration);
        let end = end.max(start).min(duration.max(start));
        last_start = start;
        timings.push(LineTiming { id: l.id, start, end });
    }

    let mut shifts: Vec<f64> = lines.iter().zip(&timings).map(|(l, t)| (t.start - l.start).abs()).collect();
    shifts.sort_by(f64::total_cmp);
    Aligned {
        matched: placed.iter().filter(|p| p.is_some()).count(),
        moved: shifts.iter().filter(|&&s| s > 1.0).count(),
        max_shift: shifts.last().copied().unwrap_or(0.0),
        timings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: [&str; 12] = [
        "So nandito na tayo sa CRM natin, nasa lead tayo.",
        "Kanan, quotation management.",
        "Sa quotation management mo, para kasi mag-quote minsan kailangan mo ng ocular.",
        "Meron na ba tayo for ocular na module diyan?",
        "Ocular, wait lang sir, check ko.",
        "Ano exactly sir module yung nagko-connect?",
        "From CRM to opportunity.",
        "Oo.",
        "'Yung kaninang process na meron kay Ariel.",
        "Parang ang ginagawa doon in-assess natin yung client.",
        "Tapos kinukuha na natin yung inquiry niya.",
        "Sa module ng CRM.",
    ];

    /// Reference words spoken from t=100 s at 0.4 s per word, with a second of pause between lines.
    fn reference() -> (Vec<Word>, Vec<(f64, f64)>) {
        let (mut words, mut spans, mut t) = (Vec::new(), Vec::new(), 100.0);
        for line in TEXT {
            let start = t;
            for w in line.split_whitespace() {
                words.push(Word { start: t, end: t + 0.35, text: w.to_string() });
                t += 0.4;
            }
            spans.push((start, t - 0.05));
            t += 1.0;
        }
        (words, spans)
    }

    fn row(i: usize, start: f64, end: f64, text: &str) -> SegmentRow {
        SegmentRow { id: i as i64 + 1, idx: i as i64, start, end, speaker: None, text: text.into() }
    }

    #[test]
    fn moves_drifted_lines_back_to_their_words() {
        let (words, spans) = reference();
        // The first half is right; the second half was written 30 s too late (drift).
        let lines: Vec<SegmentRow> = TEXT
            .iter()
            .zip(&spans)
            .enumerate()
            .map(|(i, (t, &(s, e)))| if i < 6 { row(i, s, e, t) } else { row(i, s + 30.0, e + 30.0, t) })
            .collect();
        let a = align(&lines, &words, 1000.0);
        for (t, &(s, e)) in a.timings.iter().zip(&spans) {
            assert!((t.start - s).abs() < 1.0, "line {} starts at {} not {s}", t.id, t.start);
            assert!((t.end - e).abs() < 1.0, "line {} ends at {} not {e}", t.id, t.end);
        }
        assert!(a.matched >= 10, "{a:?}");
        assert_eq!(a.moved, 6);
        assert!(a.max_shift > 29.0 && a.max_shift < 31.0);
    }

    #[test]
    fn unmatched_lines_follow_their_neighbours() {
        let (words, spans) = reference();
        let mut lines: Vec<SegmentRow> =
            TEXT.iter().zip(&spans).enumerate().map(|(i, (t, &(s, e)))| row(i, s - 20.0, e - 20.0, t)).collect();
        // Text the recognizer never heard: placed by the shift around it.
        lines[4].text = "Xylophone zebra quartz.".into();
        let a = align(&lines, &words, 1000.0);
        assert!((a.timings[4].start - spans[4].0).abs() < 1.0, "{:?}", a.timings[4]);
        assert!(a.timings.windows(2).all(|w| w[0].start <= w[1].start));
    }

    #[test]
    fn spreads_lines_that_were_given_the_same_time() {
        let (words, spans) = reference();
        let mut lines: Vec<SegmentRow> = TEXT.iter().zip(&spans).enumerate().map(|(i, (t, &(s, e)))| row(i, s, e, t)).collect();
        // Lines 7–9 were all stamped with line 10's time and the recognizer heard them differently.
        for (i, text) in [(7, "Yes."), (8, "That process from before, with Ariel."), (9, "They assess the client there.")] {
            lines[i].text = text.into();
            (lines[i].start, lines[i].end) = (spans[10].0, spans[10].0);
        }
        let a = align(&lines, &words, 1000.0);
        for i in 7..=9 {
            assert!((a.timings[i].start - spans[i].0).abs() < 2.0, "line {i}: {:?} vs {:?}", a.timings[i], spans[i]);
        }
    }

    #[test]
    fn trusts_a_well_matched_line_even_when_its_error_differs() {
        let (words, spans) = reference();
        // The model put line 8 40 s early while its neighbours are only 10 s late.
        let lines: Vec<SegmentRow> = TEXT
            .iter()
            .zip(&spans)
            .enumerate()
            .map(|(i, (t, &(s, e)))| if i == 8 { row(i, s - 40.0, e - 40.0, t) } else { row(i, s + 10.0, e + 10.0, t) })
            .collect();
        let a = align(&lines, &words, 1000.0);
        assert!((a.timings[8].start - spans[8].0).abs() < 1.0, "{:?} vs {:?}", a.timings[8], spans[8]);
    }

    #[test]
    fn ignores_a_few_common_words_matched_far_away() {
        // The recognizer heard lines 0–3 and then (after a stretch it looped through) only "na yung
        // na" two minutes later; line 4 is long and shares just those common words.
        let mut words = Vec::new();
        let mut t = 100.0;
        for line in &TEXT[..4] {
            for w in line.split_whitespace() {
                words.push(Word { start: t, end: t + 0.35, text: w.to_string() });
                t += 0.4;
            }
            t += 1.0;
        }
        for (k, w) in ["na", "yung", "na"].iter().enumerate() {
            words.push(Word { start: 240.0 + k as f64 * 0.4, end: 240.3 + k as f64 * 0.4, text: w.to_string() });
        }
        let mut lines: Vec<SegmentRow> = (0..4).map(|i| row(i, words[0].start, 0.0, TEXT[i])).collect();
        let mut at = 100.0;
        for (i, l) in lines.iter_mut().enumerate() {
            l.start = at;
            at += TEXT[i].split_whitespace().count() as f64 * 0.4 + 1.0;
            l.end = at - 1.0;
        }
        lines.push(row(4, at + 2.0, at + 12.0, "Kasi na nandito yung mga files natin na kailangan i-review bago mag-quote sa client na yan."));
        let a = align(&lines, &words, 1000.0);
        assert!((a.timings[4].start - lines[4].start).abs() < 1.0, "{:?}", a.timings[4]);
    }

    #[test]
    fn leaves_correct_lines_alone() {
        let (words, spans) = reference();
        let lines: Vec<SegmentRow> = TEXT.iter().zip(&spans).enumerate().map(|(i, (t, &(s, e)))| row(i, s, e, t)).collect();
        let a = align(&lines, &words, 1000.0);
        assert_eq!(a.moved, 0, "{a:?}");
    }

    /// Real-data check against a `whisper-cli -oj` transcript of the same audio (`ALIGN_WHISPER`), or
    /// the reference words a Fix timestamps run saved (`ALIGN_WORDS=raw/<id>/align/words.json`):
    /// `ALIGN_DB=<copy of library.sqlite> ALIGN_ID=<transcript id> ALIGN_WHISPER=<json> ALIGN_OUT=<json>
    /// cargo test real_transcript -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_transcript() {
        let env = |k: &str| std::env::var(k).unwrap();
        let c = rusqlite::Connection::open(env("ALIGN_DB")).unwrap();
        // ALIGN_VERSION=<n> aligns that archived version instead of the current one.
        let lines: Vec<SegmentRow> = match std::env::var("ALIGN_VERSION") {
            Ok(n) => {
                let json: String = c
                    .query_row(
                        "SELECT segments_json FROM transcript_versions WHERE transcript_id = ?1 AND number = ?2",
                        rusqlite::params![env("ALIGN_ID"), n.parse::<i64>().unwrap()],
                        |r| r.get(0),
                    )
                    .unwrap();
                let mut rows: Vec<SegmentRow> = serde_json::from_str(&json).unwrap();
                for (i, r) in rows.iter_mut().enumerate() {
                    r.id = i as i64;
                }
                rows
            }
            Err(_) => crate::db::segments_for(&c, &env("ALIGN_ID")).unwrap(),
        };
        let words: Vec<Word> = match std::env::var("ALIGN_WORDS") {
            Ok(p) => serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap(),
            Err(_) => {
                let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(env("ALIGN_WHISPER")).unwrap()).unwrap();
                let ms = |s: &serde_json::Value, k: &str| s["offsets"][k].as_f64().unwrap() / 1000.0;
                let segs: Vec<Segment> = v["transcription"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| Segment { start: ms(s, "from"), end: ms(s, "to"), text: s["text"].as_str().unwrap().into(), speaker: None })
                    .collect();
                reference_words(segs, vec![])
            }
        };
        let t = std::time::Instant::now();
        let a = align(&lines, &words, 1e9);
        println!("{} of {} lines matched, {} moved, max {:.1}s, in {:?}", a.matched, lines.len(), a.moved, a.max_shift, t.elapsed());
        let out: Vec<_> = lines
            .iter()
            .zip(&a.timings)
            .map(|(l, t)| serde_json::json!({"idx": l.idx, "old": l.start, "start": t.start, "end": t.end, "text": l.text}))
            .collect();
        std::fs::write(env("ALIGN_OUT"), serde_json::to_string(&out).unwrap()).unwrap();
    }

    #[test]
    fn tokens_ignore_spelling_variants() {
        assert_eq!(tokens("'Yung kaninang process, 'yon na!"), ["yung", "kaninang", "process", "yun", "na"]);
        let w = words_from_segments(&[Segment { start: 10.0, end: 12.0, text: "ab cd".into(), speaker: None }]);
        assert_eq!((w.len(), w[0].start, w[1].end), (2, 10.0, 12.0));
    }

    #[test]
    fn drops_words_whisper_produced_while_looping() {
        let seg = |s: f64, t: &str| Segment { start: s, end: s + 1.0, text: t.into(), speaker: None };
        let word = |s: f64, t: &str| Word { start: s, end: s + 0.4, text: t.into() };
        let segs = vec![seg(0.0, "Sino yung mga sampling na?"), seg(1.0, "Sino yung mga sampling na?"), seg(2.0, "Real talk here.")];
        let words = vec![word(0.1, "Sino"), word(1.1, "Sino"), word(2.1, "Real"), word(2.5, "talk")];
        let kept: Vec<f64> = reference_words(segs, words).iter().map(|w| w.start).collect();
        assert_eq!(kept, [0.1, 2.1, 2.5]);
    }
}
