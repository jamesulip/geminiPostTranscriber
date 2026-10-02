use crate::db::TranscriptDetail;
use std::collections::HashMap;

fn speaker_names(t: &TranscriptDetail) -> HashMap<&str, &str> {
    t.speakers.iter().map(|s| (s.label.as_str(), s.display_name.as_str())).collect()
}

fn stamp(t: f64, sep: char, hours: bool) -> String {
    let ms = (t * 1000.0).round() as u64;
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if hours || h > 0 {
        format!("{h:02}:{m:02}:{s:02}{sep}{ms:03}")
    } else {
        format!("{m:02}:{s:02}{sep}{ms:03}")
    }
}

fn clock(t: f64) -> String {
    let s = t as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

pub fn render(t: &TranscriptDetail, format: &str) -> String {
    let names = speaker_names(t);
    let who = |sp: &Option<String>| sp.as_deref().map(|l| names.get(l).copied().unwrap_or(l).to_string());
    let mut out = String::new();
    match format {
        "srt" => {
            for (i, s) in t.segments.iter().enumerate() {
                let prefix = who(&s.speaker).map(|w| format!("{w}: ")).unwrap_or_default();
                out += &format!(
                    "{}\n{} --> {}\n{}{}\n\n",
                    i + 1,
                    stamp(s.start, ',', true),
                    stamp(s.end, ',', true),
                    prefix,
                    s.text
                );
            }
        }
        "vtt" => {
            out += "WEBVTT\n\n";
            for s in &t.segments {
                let text = match who(&s.speaker) {
                    Some(w) => format!("<v {w}>{}", s.text),
                    None => s.text.clone(),
                };
                out += &format!("{} --> {}\n{}\n\n", stamp(s.start, '.', true), stamp(s.end, '.', true), text);
            }
        }
        "json" => {
            let segs: Vec<_> = t
                .segments
                .iter()
                .map(|s| serde_json::json!({"start": s.start, "end": s.end, "speaker": who(&s.speaker), "text": s.text}))
                .collect();
            out = serde_json::to_string_pretty(&serde_json::json!({
                "title": t.summary.title,
                "duration": t.summary.duration,
                "recorded_at": t.summary.recorded_at,
                "transcribed_at": t.summary.created_at,
                "language": t.summary.language,
                "provider": t.summary.provider,
                "model": t.summary.model,
                "version": t.summary.version,
                "segments": segs
            }))
            .unwrap_or_default();
        }
        "md" => {
            out += &format!("# {}\n\n", t.summary.title);
            let date = t.summary.recorded_at.as_deref().unwrap_or(&t.summary.created_at);
            out += &format!("- **Recorded:** {}\n", date.get(..10).unwrap_or(date));
            out += &format!("- **Duration:** {}\n", clock(t.summary.duration));
            out += &format!("- **Language:** {}\n", t.summary.language.as_deref().unwrap_or("auto"));
            out += &format!("- **Model:** {} ({}) · version {}\n\n", t.summary.model, t.summary.provider, t.summary.version);
            let mut last: Option<String> = None;
            for s in &t.segments {
                let w = who(&s.speaker);
                if w != last {
                    out += &format!("\n**{}** · {}\n\n", w.clone().unwrap_or_else(|| "Unknown".into()), clock(s.start));
                    last = w;
                }
                out += &format!("{} ", s.text);
            }
            out.push('\n');
        }
        _ => {
            for s in &t.segments {
                match who(&s.speaker) {
                    Some(w) => out += &format!("[{}] {}: {}\n", clock(s.start), w, s.text),
                    None => out += &format!("[{}] {}\n", clock(s.start), s.text),
                }
            }
        }
    }
    out
}
