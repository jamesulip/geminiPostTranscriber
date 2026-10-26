//! Line-by-line rewrites (translation, clean-up): each `[time] Name: text` line of a part is sent
//! with its segment id (`#12 [0:05] Juan: …`) and the reply is matched back by that id, so a line
//! the model merges, skips or adds can't shift every later line onto the wrong segment.

use std::collections::BTreeMap;

/// Prefix each line of a chunk's text with its segment id, starting at `first_idx`.
pub fn with_ids(text: &str, first_idx: i64) -> String {
    source_lines(text).iter().enumerate().map(|(i, l)| format!("#{} {l}", first_idx + i as i64)).collect::<Vec<_>>().join("\n")
}

/// Map the model's reply back onto segment ids. Lines starting `#id` are matched by id (ids
/// outside the part are ignored; the first answer for an id wins). A reply with no ids at all
/// falls back to matching lines in order.
pub fn parse(source: &str, reply: &str, first_idx: i64) -> BTreeMap<i64, String> {
    let src = source_lines(source);
    let id_line = regex::Regex::new(r"^\s*#(\d+)\s?(.*)$").unwrap();
    let mut map = BTreeMap::new();
    for line in reply.lines() {
        let Some(c) = id_line.captures(line) else { continue };
        let Ok(idx) = c[1].parse::<i64>() else { continue };
        let Some(src_line) = usize::try_from(idx - first_idx).ok().and_then(|i| src.get(i)) else { continue };
        map.entry(idx).or_insert_with(|| strip_prefix(&c[2], src_line));
    }
    if map.is_empty() {
        let out: Vec<&str> = reply.lines().filter(|l| !l.trim().is_empty()).collect();
        for (i, (line, src_line)) in out.iter().zip(&src).enumerate() {
            map.insert(first_idx + i as i64, strip_prefix(line, src_line));
        }
    }
    map
}

fn source_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| !l.trim().is_empty()).collect()
}

/// Drop the `[time] Name:` prefix the prompt asked the model to repeat: exactly as given when it
/// was, otherwise a `[time]` and a short `Name:` (so colons inside the speech survive).
fn strip_prefix(line: &str, src_line: &str) -> String {
    let line = line.trim();
    let (src_time, src_rest) = split_time(src_line.trim());
    let (_, rest) = split_time(line);
    if src_time.is_some() {
        if let Some((who, _)) = src_rest.split_once(": ").filter(|(who, _)| who.chars().count() <= 40) {
            if let Some(text) = rest.strip_prefix(who).and_then(|r| r.strip_prefix(':')) {
                return text.trim().to_string();
            }
        }
    }
    match rest.split_once(": ") {
        Some((who, text)) if who.chars().count() <= 40 && who.split(' ').count() <= 3 => text.trim().to_string(),
        _ => rest.to_string(),
    }
}

/// "[0:05] rest" → (Some("0:05"), "rest").
fn split_time(l: &str) -> (Option<&str>, &str) {
    match l.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((t, rest)) => (Some(t), rest.trim_start()),
        None => (None, l),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "[0:00] Maria: Magandang umaga.\n[0:05] Juan: Simulan na natin.\n[0:09] Maria: Sige.\n";

    #[test]
    fn prefixes_lines_with_segment_ids() {
        assert_eq!(with_ids(SRC, 10), "#10 [0:00] Maria: Magandang umaga.\n#11 [0:05] Juan: Simulan na natin.\n#12 [0:09] Maria: Sige.");
    }

    #[test]
    fn matches_by_id_so_a_skipped_line_shifts_nothing() {
        let out = "#10 [0:00] Maria: Good morning.\n#12 [0:09] Maria: Okay: fine.\n#99 [9:99] X: stray\nnote without id";
        let m = parse(SRC, out, 10);
        assert_eq!(m.get(&10).map(String::as_str), Some("Good morning."));
        assert_eq!(m.get(&11), None);
        assert_eq!(m.get(&12).map(String::as_str), Some("Okay: fine."));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn keeps_the_first_answer_and_reads_dashes() {
        let m = parse(SRC, "#11 [0:05] Juan: -\n#11 [0:05] Juan: again\n#10 Good morning.", 10);
        assert_eq!(m.get(&11).map(String::as_str), Some("-"));
        assert_eq!(m.get(&10).map(String::as_str), Some("Good morning."));
    }

    #[test]
    fn strips_long_speaker_names_given_exactly() {
        let src = "[1:00] Maria Clara de los Santos: Oo.";
        let m = parse(src, "#0 [1:00] Maria Clara de los Santos: Yes.", 0);
        assert_eq!(m.get(&0).map(String::as_str), Some("Yes."));
    }

    #[test]
    fn falls_back_to_order_without_ids() {
        let out = "[0:00] Maria: Good morning.\n[0:05] Juan: Let's start: now.\n[0:09] Maria: Okay.\nExtra line";
        let m = parse(SRC, out, 10);
        assert_eq!(m.get(&10).map(String::as_str), Some("Good morning."));
        assert_eq!(m.get(&11).map(String::as_str), Some("Let's start: now."));
        assert_eq!(m.len(), 3);
    }
}
