use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Additive migrations for libraries created by earlier versions.
fn migrate(c: &Connection) -> Result<()> {
    let cols: Vec<String> = c
        .prepare("SELECT name FROM pragma_table_info('transcripts')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for (name, ty) in [
        ("recorded_at", "TEXT"),
        ("detected_language", "TEXT"),
        ("version", "INTEGER NOT NULL DEFAULT 1"),
        ("transcribed_at", "TEXT"),
    ] {
        if !cols.iter().any(|c| c == name) {
            c.execute_batch(&format!("ALTER TABLE transcripts ADD COLUMN {name} {ty}"))?;
        }
    }
    Ok(())
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  parent_id INTEGER REFERENCES folders(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS tags (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  color TEXT NOT NULL DEFAULT '#64748b'
);

CREATE TABLE IF NOT EXISTS transcripts (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL,
  audio_path TEXT NOT NULL,
  source_name TEXT NOT NULL,
  duration REAL NOT NULL DEFAULT 0,
  processed_duration REAL NOT NULL DEFAULT 0,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  language TEXT,
  status TEXT NOT NULL,
  error TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS transcript_tags (
  transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
  tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (transcript_id, tag_id)
);

CREATE TABLE IF NOT EXISTS segments (
  id INTEGER PRIMARY KEY,
  transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL,
  start REAL NOT NULL,
  end REAL NOT NULL,
  speaker TEXT,
  text TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS segments_transcript ON segments(transcript_id, idx);

CREATE TABLE IF NOT EXISTS speakers (
  transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
  label TEXT NOT NULL,
  display_name TEXT NOT NULL,
  PRIMARY KEY (transcript_id, label)
);

-- Earlier transcriptions of a transcript. The current version lives in segments/speakers.
CREATE TABLE IF NOT EXISTS transcript_versions (
  id INTEGER PRIMARY KEY,
  transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
  number INTEGER NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  language TEXT,
  processed_duration REAL NOT NULL,
  transcribed_at TEXT NOT NULL,
  archived_at TEXT NOT NULL,
  segments_json TEXT NOT NULL,
  speakers_json TEXT NOT NULL,
  UNIQUE (transcript_id, number)
);

-- One row per transcription job (kept when the transcript is deleted).
CREATE TABLE IF NOT EXISTS usage (
  id INTEGER PRIMARY KEY,
  transcript_id TEXT REFERENCES transcripts(id) ON DELETE SET NULL,
  title TEXT NOT NULL,
  provider TEXT NOT NULL,
  provider_name TEXT NOT NULL,
  model TEXT NOT NULL,
  status TEXT NOT NULL,
  original_seconds REAL NOT NULL,
  sent_seconds REAL NOT NULL,
  requests INTEGER NOT NULL,
  input_tokens INTEGER NOT NULL,
  output_tokens INTEGER NOT NULL,
  price_per_minute REAL NOT NULL,
  cost_usd REAL NOT NULL,
  saved_usd REAL NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS usage_transcript ON usage(transcript_id);
CREATE INDEX IF NOT EXISTS usage_created ON usage(created_at);

CREATE VIRTUAL TABLE IF NOT EXISTS segments_fts USING fts5(
  text, content='segments', content_rowid='id', tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS segments_ai AFTER INSERT ON segments BEGIN
  INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS segments_ad AFTER DELETE ON segments BEGIN
  INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER IF NOT EXISTS segments_au AFTER UPDATE ON segments BEGIN
  INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
  INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;
"#;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct TranscriptSummary {
    pub id: String,
    pub title: String,
    pub folder_id: Option<i64>,
    pub source_name: String,
    pub duration: f64,
    pub processed_duration: f64,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub tags: Vec<Tag>,
    pub preview: Option<String>,
    /// When the audio was recorded (file metadata, else file modification time).
    pub recorded_at: Option<String>,
    /// Requested language, else the language the provider detected.
    pub language: Option<String>,
    /// Version number of the current transcription.
    pub version: i64,
    /// When the current version was produced (None for libraries from before versioning).
    pub transcribed_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SegmentRow {
    pub id: i64,
    pub idx: i64,
    pub start: f64,
    pub end: f64,
    pub speaker: Option<String>,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Speaker {
    pub label: String,
    pub display_name: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct TranscriptDetail {
    #[serde(flatten)]
    pub summary: TranscriptSummary,
    pub audio_path: String,
    pub segments: Vec<SegmentRow>,
    pub speakers: Vec<Speaker>,
    pub usage: UsageTotals,
}

#[derive(Debug, Serialize, Clone)]
pub struct SearchHit {
    pub transcript_id: String,
    pub transcript_title: String,
    pub segment_id: i64,
    pub start: f64,
    pub speaker: Option<String>,
    pub snippet: String,
}

pub struct NewTranscript<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub folder_id: Option<i64>,
    pub audio_path: &'a str,
    pub source_name: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub language: Option<&'a str>,
    pub recorded_at: Option<&'a str>,
    pub tag_ids: &'a [i64],
}

pub fn insert_transcript(c: &Connection, t: &NewTranscript) -> Result<()> {
    c.execute(
        "INSERT INTO transcripts (id, title, folder_id, audio_path, source_name, provider, model, language, status, created_at, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'processing', ?9, ?10)",
        params![
            t.id, t.title, t.folder_id, t.audio_path, t.source_name, t.provider, t.model, t.language,
            chrono::Local::now().to_rfc3339(), t.recorded_at
        ],
    )?;
    set_tags(c, t.id, t.tag_ids)?;
    Ok(())
}

pub fn set_status(c: &Connection, id: &str, status: &str, error: Option<&str>) -> Result<()> {
    c.execute("UPDATE transcripts SET status = ?2, error = ?3 WHERE id = ?1", params![id, status, error])?;
    Ok(())
}

/// Store recording metadata found while processing. `None` values leave existing data untouched.
pub fn set_recording_meta(c: &Connection, id: &str, recorded_at: Option<&str>, detected_language: Option<&str>) -> Result<()> {
    c.execute(
        "UPDATE transcripts SET recorded_at = COALESCE(?2, recorded_at), detected_language = COALESCE(?3, detected_language)
         WHERE id = ?1",
        params![id, recorded_at, detected_language],
    )?;
    Ok(())
}

pub fn set_duration(c: &Connection, id: &str, duration: f64) -> Result<()> {
    c.execute("UPDATE transcripts SET duration = ?2 WHERE id = ?1", params![id, duration])?;
    Ok(())
}

fn insert_segments(tx: &rusqlite::Transaction, id: &str, rows: &[SegmentRow], speakers: &[Speaker]) -> Result<()> {
    tx.execute("DELETE FROM segments WHERE transcript_id = ?1", [id])?;
    tx.execute("DELETE FROM speakers WHERE transcript_id = ?1", [id])?;
    let mut st =
        tx.prepare("INSERT INTO segments (transcript_id, idx, start, end, speaker, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
    for (i, s) in rows.iter().enumerate() {
        st.execute(params![id, i as i64, s.start, s.end, s.speaker, s.text])?;
    }
    let mut sp = tx.prepare(
        "INSERT INTO speakers (transcript_id, label, display_name) VALUES (?1, ?2, ?3)
         ON CONFLICT(transcript_id, label) DO UPDATE SET display_name = excluded.display_name",
    )?;
    for s in speakers {
        sp.execute(params![id, s.label, s.display_name])?;
    }
    for label in rows.iter().filter_map(|s| s.speaker.as_deref()) {
        tx.execute(
            "INSERT OR IGNORE INTO speakers (transcript_id, label, display_name) VALUES (?1, ?2, ?2)",
            params![id, label],
        )?;
    }
    Ok(())
}

/// Copy the current transcription into `transcript_versions`. Returns false if there was nothing to archive.
fn archive_current(tx: &rusqlite::Transaction, id: &str) -> Result<bool> {
    let segments = segments_for(tx, id)?;
    if segments.is_empty() {
        return Ok(false);
    }
    let speakers = speakers_for(tx, id)?;
    tx.execute(
        "INSERT INTO transcript_versions (transcript_id, number, provider, model, language, processed_duration,
           transcribed_at, archived_at, segments_json, speakers_json)
         SELECT id, version, provider, model, COALESCE(NULLIF(language, ''), detected_language), processed_duration,
                COALESCE(transcribed_at, created_at), ?2, ?3, ?4
         FROM transcripts WHERE id = ?1",
        params![
            id,
            chrono::Local::now().to_rfc3339(),
            serde_json::to_string(&segments)?,
            serde_json::to_string(&speakers)?
        ],
    )?;
    Ok(true)
}

pub struct CompletedVersion<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub language: Option<&'a str>,
    pub detected_language: Option<&'a str>,
    pub processed_duration: f64,
}

/// Store a finished transcription as the current version, archiving the previous one.
pub fn commit_version(c: &mut Connection, id: &str, segs: &[crate::providers::Segment], v: &CompletedVersion) -> Result<()> {
    let tx = c.transaction()?;
    let archived = archive_current(&tx, id)?;
    let rows: Vec<SegmentRow> = segs
        .iter()
        .enumerate()
        .map(|(i, s)| SegmentRow {
            id: 0,
            idx: i as i64,
            start: s.start,
            end: s.end,
            speaker: s.speaker.clone(),
            text: s.text.clone(),
        })
        .collect();
    insert_segments(&tx, id, &rows, &[])?;
    let number: i64 = if archived {
        tx.query_row(
            "SELECT MAX((SELECT version FROM transcripts WHERE id = ?1),
                        COALESCE((SELECT MAX(number) FROM transcript_versions WHERE transcript_id = ?1), 0)) + 1",
            [id],
            |r| r.get(0),
        )?
    } else {
        tx.query_row("SELECT version FROM transcripts WHERE id = ?1", [id], |r| r.get(0))?
    };
    tx.execute(
        "UPDATE transcripts SET version = ?2, provider = ?3, model = ?4, language = ?5, detected_language = ?6,
           processed_duration = ?7, transcribed_at = ?8, error = NULL WHERE id = ?1",
        params![
            id, number, v.provider, v.model, v.language, v.detected_language, v.processed_duration,
            chrono::Local::now().to_rfc3339()
        ],
    )?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Serialize, Clone)]
pub struct VersionInfo {
    pub id: i64,
    pub number: i64,
    pub provider: String,
    pub model: String,
    pub language: Option<String>,
    pub processed_duration: f64,
    pub transcribed_at: String,
    pub segment_count: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct VersionDetail {
    #[serde(flatten)]
    pub info: VersionInfo,
    pub segments: Vec<SegmentRow>,
    pub speakers: Vec<Speaker>,
}

const VERSION_COLS: &str =
    "id, number, provider, model, language, processed_duration, transcribed_at, json_array_length(segments_json)";

fn version_from_row(r: &rusqlite::Row) -> rusqlite::Result<VersionInfo> {
    Ok(VersionInfo {
        id: r.get(0)?,
        number: r.get(1)?,
        provider: r.get(2)?,
        model: r.get(3)?,
        language: r.get(4)?,
        processed_duration: r.get(5)?,
        transcribed_at: r.get(6)?,
        segment_count: r.get(7)?,
    })
}

/// Archived (non-current) versions, newest first.
pub fn list_versions(c: &Connection, id: &str) -> Result<Vec<VersionInfo>> {
    let mut st = c.prepare(&format!(
        "SELECT {VERSION_COLS} FROM transcript_versions WHERE transcript_id = ?1 ORDER BY number DESC"
    ))?;
    let rows = st.query_map([id], version_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get_version(c: &Connection, version_id: i64) -> Result<Option<VersionDetail>> {
    let row = c
        .query_row(
            &format!("SELECT {VERSION_COLS}, segments_json, speakers_json FROM transcript_versions WHERE id = ?1"),
            [version_id],
            |r| Ok((version_from_row(r)?, r.get::<_, String>(8)?, r.get::<_, String>(9)?)),
        )
        .optional()?;
    let Some((info, segs, speakers)) = row else { return Ok(None) };
    let mut segments: Vec<SegmentRow> = serde_json::from_str(&segs)?;
    // Archived segments have no live row id; give them stable negative ids for the UI.
    for (i, s) in segments.iter_mut().enumerate() {
        s.id = -(i as i64) - 1;
    }
    Ok(Some(VersionDetail { info, segments, speakers: serde_json::from_str(&speakers)? }))
}

/// Make an archived version current again; the current one is archived in its place.
pub fn restore_version(c: &mut Connection, version_id: i64) -> Result<()> {
    let tx = c.transaction()?;
    let (id, number, provider, model, language, processed, transcribed_at, segs, speakers): (
        String, i64, String, String, Option<String>, f64, String, String, String,
    ) = tx.query_row(
        "SELECT transcript_id, number, provider, model, language, processed_duration, transcribed_at, segments_json,
                speakers_json FROM transcript_versions WHERE id = ?1",
        [version_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)),
    )?;
    tx.execute("DELETE FROM transcript_versions WHERE id = ?1", [version_id])?;
    archive_current(&tx, &id)?;
    let rows: Vec<SegmentRow> = serde_json::from_str(&segs)?;
    let speakers: Vec<Speaker> = serde_json::from_str(&speakers)?;
    insert_segments(&tx, &id, &rows, &speakers)?;
    tx.execute(
        "UPDATE transcripts SET version = ?2, provider = ?3, model = ?4, language = ?5, detected_language = NULL,
           processed_duration = ?6, transcribed_at = ?7, status = 'done', error = NULL WHERE id = ?1",
        params![id, number, provider, model, language, processed, transcribed_at],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn delete_version(c: &Connection, version_id: i64) -> Result<()> {
    c.execute("DELETE FROM transcript_versions WHERE id = ?1", [version_id])?;
    Ok(())
}

/// A job ended with an error. Keep the transcript usable if it still has a previous version.
pub fn mark_failed(c: &Connection, id: &str, error: &str) -> Result<()> {
    c.execute(
        "UPDATE transcripts SET error = ?2,
           status = CASE WHEN EXISTS (SELECT 1 FROM segments WHERE transcript_id = ?1) THEN 'done' ELSE 'error' END
         WHERE id = ?1",
        params![id, error],
    )?;
    Ok(())
}

/// Mark transcripts left in 'processing' by a previous run (app quit mid-job) as failed.
pub fn fail_stale(c: &Connection) -> Result<()> {
    c.execute(
        "UPDATE transcripts SET error = 'Interrupted (app was closed)',
           status = CASE WHEN EXISTS (SELECT 1 FROM segments s WHERE s.transcript_id = transcripts.id) THEN 'done' ELSE 'error' END
         WHERE status = 'processing'",
        [],
    )?;
    Ok(())
}

fn tags_for(c: &Connection, id: &str) -> Result<Vec<Tag>> {
    let mut st = c.prepare(
        "SELECT t.id, t.name, t.color FROM tags t JOIN transcript_tags tt ON tt.tag_id = t.id
         WHERE tt.transcript_id = ?1 ORDER BY t.name",
    )?;
    let rows = st.query_map([id], |r| Ok(Tag { id: r.get(0)?, name: r.get(1)?, color: r.get(2)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

const SUMMARY_COLS: &str = "t.id, t.title, t.folder_id, t.source_name, t.duration, t.processed_duration, t.provider,
    t.model, t.status, t.error, t.created_at,
    (SELECT substr(group_concat(text, ' '), 1, 200) FROM (SELECT text FROM segments s WHERE s.transcript_id = t.id ORDER BY idx LIMIT 3)),
    t.recorded_at, COALESCE(NULLIF(t.language, ''), t.detected_language), t.version, t.transcribed_at";

fn summary_from_row(r: &rusqlite::Row) -> rusqlite::Result<TranscriptSummary> {
    Ok(TranscriptSummary {
        id: r.get(0)?,
        title: r.get(1)?,
        folder_id: r.get(2)?,
        source_name: r.get(3)?,
        duration: r.get(4)?,
        processed_duration: r.get(5)?,
        provider: r.get(6)?,
        model: r.get(7)?,
        status: r.get(8)?,
        error: r.get(9)?,
        created_at: r.get(10)?,
        tags: vec![],
        preview: r.get(11)?,
        recorded_at: r.get(12)?,
        language: r.get(13)?,
        version: r.get(14)?,
        transcribed_at: r.get(15)?,
    })
}

#[derive(Debug, Deserialize, Default)]
pub struct ListFilter {
    pub folder_id: Option<i64>,
    pub tag_id: Option<i64>,
    /// Include transcripts in sub-folders of `folder_id`.
    #[serde(default)]
    pub recursive: bool,
}

pub fn list_transcripts(c: &Connection, f: &ListFilter) -> Result<Vec<TranscriptSummary>> {
    let sql = format!(
        "WITH RECURSIVE sub(id) AS (
           SELECT ?1 WHERE ?1 IS NOT NULL
           UNION SELECT folders.id FROM folders JOIN sub ON folders.parent_id = sub.id WHERE ?3
         )
         SELECT {SUMMARY_COLS} FROM transcripts t
         WHERE (?1 IS NULL OR t.folder_id IN (SELECT id FROM sub))
           AND (?2 IS NULL OR EXISTS (SELECT 1 FROM transcript_tags tt WHERE tt.transcript_id = t.id AND tt.tag_id = ?2))
         ORDER BY t.created_at DESC"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![f.folder_id, f.tag_id, f.recursive], summary_from_row)?;
    let mut out: Vec<TranscriptSummary> = rows.collect::<Result<_, _>>()?;
    for t in &mut out {
        t.tags = tags_for(c, &t.id)?;
    }
    Ok(out)
}

fn segments_for(c: &Connection, id: &str) -> Result<Vec<SegmentRow>> {
    let mut st =
        c.prepare("SELECT id, idx, start, end, speaker, text FROM segments WHERE transcript_id = ?1 ORDER BY idx")?;
    let rows = st.query_map([id], |r| {
        Ok(SegmentRow { id: r.get(0)?, idx: r.get(1)?, start: r.get(2)?, end: r.get(3)?, speaker: r.get(4)?, text: r.get(5)? })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn speakers_for(c: &Connection, id: &str) -> Result<Vec<Speaker>> {
    let mut st = c.prepare("SELECT label, display_name FROM speakers WHERE transcript_id = ?1 ORDER BY label")?;
    let rows = st.query_map([id], |r| Ok(Speaker { label: r.get(0)?, display_name: r.get(1)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get_transcript(c: &Connection, id: &str) -> Result<Option<TranscriptDetail>> {
    let sql = format!("SELECT {SUMMARY_COLS}, t.audio_path FROM transcripts t WHERE t.id = ?1");
    let row = c.query_row(&sql, [id], |r| Ok((summary_from_row(r)?, r.get::<_, String>(16)?))).optional()?;
    let Some((mut summary, audio_path)) = row else { return Ok(None) };
    summary.tags = tags_for(c, id)?;

    let segments = segments_for(c, id)?;
    let speakers = speakers_for(c, id)?;
    let usage = usage_for_transcript(c, id)?;
    Ok(Some(TranscriptDetail { summary, audio_path, segments, speakers, usage }))
}

pub fn update_transcript(c: &Connection, id: &str, title: &str, folder_id: Option<i64>, tag_ids: &[i64]) -> Result<()> {
    c.execute("UPDATE transcripts SET title = ?2, folder_id = ?3 WHERE id = ?1", params![id, title, folder_id])?;
    set_tags(c, id, tag_ids)
}

fn set_tags(c: &Connection, id: &str, tag_ids: &[i64]) -> Result<()> {
    c.execute("DELETE FROM transcript_tags WHERE transcript_id = ?1", [id])?;
    for t in tag_ids {
        c.execute("INSERT OR IGNORE INTO transcript_tags (transcript_id, tag_id) VALUES (?1, ?2)", params![id, t])?;
    }
    Ok(())
}

pub fn delete_transcript(c: &Connection, id: &str) -> Result<Option<String>> {
    let audio: Option<String> =
        c.query_row("SELECT audio_path FROM transcripts WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    c.execute("DELETE FROM transcripts WHERE id = ?1", [id])?;
    Ok(audio)
}

pub fn rename_speaker(c: &Connection, id: &str, label: &str, name: &str) -> Result<()> {
    c.execute(
        "INSERT INTO speakers (transcript_id, label, display_name) VALUES (?1, ?2, ?3)
         ON CONFLICT(transcript_id, label) DO UPDATE SET display_name = excluded.display_name",
        params![id, label, name],
    )?;
    Ok(())
}

pub fn update_segment(c: &Connection, segment_id: i64, text: &str, speaker: Option<&str>) -> Result<()> {
    c.execute("UPDATE segments SET text = ?2, speaker = ?3 WHERE id = ?1", params![segment_id, text, speaker])?;
    Ok(())
}

/// Turn free text into a safe FTS5 prefix query: each word quoted, last word prefix-matched.
fn fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split_whitespace()
        .map(|w| w.replace('"', ""))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect();
    if words.is_empty() {
        return None;
    }
    Some(format!("{}*", words.join(" ")))
}

pub fn search(c: &Connection, q: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let Some(fq) = fts_query(q) else { return Ok(vec![]) };
    let mut st = c.prepare(
        "SELECT s.transcript_id, t.title, s.id, s.start, COALESCE(sp.display_name, s.speaker),
                snippet(segments_fts, 0, '<mark>', '</mark>', '…', 16)
         FROM segments_fts
         JOIN segments s ON s.id = segments_fts.rowid
         JOIN transcripts t ON t.id = s.transcript_id
         LEFT JOIN speakers sp ON sp.transcript_id = s.transcript_id AND sp.label = s.speaker
         WHERE segments_fts MATCH ?1
         ORDER BY rank LIMIT ?2",
    )?;
    let mut hits: Vec<SearchHit> = st
        .query_map(params![fq, limit], |r| {
            Ok(SearchHit {
                transcript_id: r.get(0)?,
                transcript_title: r.get(1)?,
                segment_id: r.get(2)?,
                start: r.get(3)?,
                speaker: r.get(4)?,
                snippet: r.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    // Title matches (shown first, pointing at the beginning).
    let like = format!("%{}%", q.trim());
    let mut st = c.prepare("SELECT id, title FROM transcripts WHERE title LIKE ?1 LIMIT 10")?;
    let titles: Vec<SearchHit> = st
        .query_map([like], |r| {
            let title: String = r.get(1)?;
            Ok(SearchHit {
                transcript_id: r.get(0)?,
                transcript_title: title.clone(),
                segment_id: -1,
                start: 0.0,
                speaker: None,
                snippet: title,
            })
        })?
        .collect::<Result<_, _>>()?;
    hits.splice(0..0, titles);
    Ok(hits)
}

// Usage ----------------------------------------------------------------------

pub struct NewUsage<'a> {
    pub transcript_id: &'a str,
    pub title: &'a str,
    pub provider: &'a str,
    pub provider_name: &'a str,
    pub model: &'a str,
    pub status: &'a str,
    pub original_seconds: f64,
    pub sent_seconds: f64,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub price_per_minute: f64,
    pub cost_usd: f64,
    pub saved_usd: f64,
}

/// Estimated (cost, saved) from audio minutes sent at a per-minute price.
pub fn minute_cost(original_seconds: f64, sent_seconds: f64, price_per_minute: f64) -> (f64, f64) {
    (
        sent_seconds / 60.0 * price_per_minute,
        (original_seconds - sent_seconds).max(0.0) / 60.0 * price_per_minute,
    )
}

/// Estimated (cost, saved) from reported tokens. Only input (audio) tokens shrink when audio is
/// condensed; output (text) tokens depend on what was said.
pub fn token_cost(input: i64, output: i64, (pin, pout): (f64, f64), original_seconds: f64, sent_seconds: f64) -> (f64, f64) {
    let input_cost = input as f64 * pin / 1e6;
    let cost = input_cost + output as f64 * pout / 1e6;
    let ratio = if sent_seconds > 0.0 { original_seconds / sent_seconds } else { 1.0 };
    (cost, input_cost * (ratio - 1.0).max(0.0))
}

pub fn insert_usage(c: &Connection, u: &NewUsage) -> Result<()> {
    let (cost, saved) = (u.cost_usd, u.saved_usd);
    c.execute(
        "INSERT INTO usage (transcript_id, title, provider, provider_name, model, status, original_seconds, sent_seconds,
           requests, input_tokens, output_tokens, price_per_minute, cost_usd, saved_usd, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            u.transcript_id, u.title, u.provider, u.provider_name, u.model, u.status, u.original_seconds,
            u.sent_seconds, u.requests, u.input_tokens, u.output_tokens, u.price_per_minute, cost, saved,
            chrono::Local::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct UsageTotals {
    pub jobs: i64,
    pub original_seconds: f64,
    pub sent_seconds: f64,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub saved_usd: f64,
}

const TOTALS_COLS: &str = "COUNT(*), COALESCE(SUM(original_seconds), 0), COALESCE(SUM(sent_seconds), 0),
    COALESCE(SUM(requests), 0), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
    COALESCE(SUM(cost_usd), 0), COALESCE(SUM(saved_usd), 0)";

fn totals_from(r: &rusqlite::Row, at: usize) -> rusqlite::Result<UsageTotals> {
    Ok(UsageTotals {
        jobs: r.get(at)?,
        original_seconds: r.get(at + 1)?,
        sent_seconds: r.get(at + 2)?,
        requests: r.get(at + 3)?,
        input_tokens: r.get(at + 4)?,
        output_tokens: r.get(at + 5)?,
        cost_usd: r.get(at + 6)?,
        saved_usd: r.get(at + 7)?,
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct UsageGroup {
    pub month: String,
    pub provider_name: String,
    pub model: String,
    #[serde(flatten)]
    pub totals: UsageTotals,
}

#[derive(Debug, Serialize, Clone)]
pub struct UsageRow {
    pub id: i64,
    pub transcript_id: Option<String>,
    pub title: String,
    pub provider_name: String,
    pub model: String,
    pub status: String,
    pub original_seconds: f64,
    pub sent_seconds: f64,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub saved_usd: f64,
    pub created_at: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct UsageSummary {
    pub all_time: UsageTotals,
    pub this_month: UsageTotals,
    pub by_month: Vec<UsageGroup>,
    pub recent: Vec<UsageRow>,
}

pub fn usage_for_transcript(c: &Connection, id: &str) -> Result<UsageTotals> {
    let sql = format!("SELECT {TOTALS_COLS} FROM usage WHERE transcript_id = ?1");
    Ok(c.query_row(&sql, [id], |r| totals_from(r, 0))?)
}

pub fn usage_summary(c: &Connection) -> Result<UsageSummary> {
    let all_time = c.query_row(&format!("SELECT {TOTALS_COLS} FROM usage"), [], |r| totals_from(r, 0))?;
    let month = chrono::Local::now().format("%Y-%m").to_string();
    let this_month = c.query_row(
        &format!("SELECT {TOTALS_COLS} FROM usage WHERE substr(created_at, 1, 7) = ?1"),
        [&month],
        |r| totals_from(r, 0),
    )?;
    let mut st = c.prepare(&format!(
        "SELECT substr(created_at, 1, 7) AS m, provider_name, model, {TOTALS_COLS} FROM usage
         GROUP BY m, provider_name, model ORDER BY m DESC, SUM(cost_usd) DESC"
    ))?;
    let by_month = st
        .query_map([], |r| {
            Ok(UsageGroup { month: r.get(0)?, provider_name: r.get(1)?, model: r.get(2)?, totals: totals_from(r, 3)? })
        })?
        .collect::<Result<_, _>>()?;
    let mut st = c.prepare(
        "SELECT id, transcript_id, title, provider_name, model, status, original_seconds, sent_seconds, requests,
                input_tokens, output_tokens, cost_usd, saved_usd, created_at
         FROM usage ORDER BY created_at DESC LIMIT 100",
    )?;
    let recent = st
        .query_map([], |r| {
            Ok(UsageRow {
                id: r.get(0)?,
                transcript_id: r.get(1)?,
                title: r.get(2)?,
                provider_name: r.get(3)?,
                model: r.get(4)?,
                status: r.get(5)?,
                original_seconds: r.get(6)?,
                sent_seconds: r.get(7)?,
                requests: r.get(8)?,
                input_tokens: r.get(9)?,
                output_tokens: r.get(10)?,
                cost_usd: r.get(11)?,
                saved_usd: r.get(12)?,
                created_at: r.get(13)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(UsageSummary { all_time, this_month, by_month, recent })
}

// Folders & tags ------------------------------------------------------------

pub fn list_folders(c: &Connection) -> Result<Vec<Folder>> {
    let mut st = c.prepare("SELECT id, name, parent_id FROM folders ORDER BY name COLLATE NOCASE")?;
    let rows = st.query_map([], |r| Ok(Folder { id: r.get(0)?, name: r.get(1)?, parent_id: r.get(2)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn create_folder(c: &Connection, name: &str, parent_id: Option<i64>) -> Result<Folder> {
    c.execute("INSERT INTO folders (name, parent_id) VALUES (?1, ?2)", params![name, parent_id])?;
    Ok(Folder { id: c.last_insert_rowid(), name: name.into(), parent_id })
}

pub fn rename_folder(c: &Connection, id: i64, name: &str) -> Result<()> {
    c.execute("UPDATE folders SET name = ?2 WHERE id = ?1", params![id, name])?;
    Ok(())
}

pub fn delete_folder(c: &Connection, id: i64) -> Result<()> {
    c.execute("DELETE FROM folders WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_tags(c: &Connection) -> Result<Vec<Tag>> {
    let mut st = c.prepare("SELECT id, name, color FROM tags ORDER BY name COLLATE NOCASE")?;
    let rows = st.query_map([], |r| Ok(Tag { id: r.get(0)?, name: r.get(1)?, color: r.get(2)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn create_tag(c: &Connection, name: &str, color: &str) -> Result<Tag> {
    c.execute("INSERT INTO tags (name, color) VALUES (?1, ?2)", params![name, color])?;
    Ok(Tag { id: c.last_insert_rowid(), name: name.into(), color: color.into() })
}

pub fn delete_tag(c: &Connection, id: i64) -> Result<()> {
    c.execute("DELETE FROM tags WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Segment;

    #[test]
    fn search_and_filter() {
        let dir = std::env::temp_dir().join(format!("transcriber-test-{}", uuid::Uuid::new_v4()));
        let mut c = open(&dir.join("db.sqlite")).unwrap();
        let parent = create_folder(&c, "Work", None).unwrap();
        let child = create_folder(&c, "Meetings", Some(parent.id)).unwrap();
        let tag = create_tag(&c, "important", "#f00").unwrap();
        insert_transcript(
            &c,
            &NewTranscript {
                id: "t1",
                title: "Standup",
                folder_id: Some(child.id),
                audio_path: "/x.ogg",
                source_name: "x.ogg",
                provider: "groq",
                model: "m",
                language: None,
                recorded_at: Some("2026-09-30T10:00:00+08:00"),
                tag_ids: &[tag.id],
            },
        )
        .unwrap();
        let segs = vec![Segment { start: 1.0, end: 2.0, text: "Quarterly revenue looks great".into(), speaker: Some("Speaker 1".into()) }];
        let v1 = CompletedVersion { provider: "groq", model: "m1", language: None, detected_language: None, processed_duration: 1.0 };
        commit_version(&mut c, "t1", &segs, &v1).unwrap();
        assert!(list_versions(&c, "t1").unwrap().is_empty());

        set_recording_meta(&c, "t1", None, Some("english")).unwrap();
        let d = get_transcript(&c, "t1").unwrap().unwrap();
        assert_eq!(d.summary.language.as_deref(), Some("english"));
        assert_eq!(d.summary.recorded_at.as_deref(), Some("2026-09-30T10:00:00+08:00"));

        let hits = search(&c, "revenu", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("<mark>"));

        let f = ListFilter { folder_id: Some(parent.id), tag_id: None, recursive: true };
        assert_eq!(list_transcripts(&c, &f).unwrap().len(), 1);
        let f = ListFilter { folder_id: Some(parent.id), tag_id: None, recursive: false };
        assert_eq!(list_transcripts(&c, &f).unwrap().len(), 0);
        let f = ListFilter { folder_id: None, tag_id: Some(tag.id), recursive: false };
        assert_eq!(list_transcripts(&c, &f).unwrap()[0].tags.len(), 1);

        let u = NewUsage {
            transcript_id: "t1", title: "Standup", provider: "groq", provider_name: "Groq", model: "m", status: "done",
            original_seconds: 600.0, sent_seconds: 240.0, requests: 1, input_tokens: 0, output_tokens: 0,
            price_per_minute: 0.006,
            cost_usd: minute_cost(600.0, 240.0, 0.006).0,
            saved_usd: minute_cost(600.0, 240.0, 0.006).1,
        };
        insert_usage(&c, &u).unwrap();
        // 1 min of audio at 25 tok/s = 1500 input tokens; 2× condensed.
        let (cost, saved) = token_cost(1500, 175, (2.0, 12.0), 120.0, 60.0);
        assert!((cost - (0.003 + 0.0021)).abs() < 1e-9 && (saved - 0.003).abs() < 1e-9);
        let t = usage_for_transcript(&c, "t1").unwrap();
        assert!((t.cost_usd - 0.024).abs() < 1e-9 && (t.saved_usd - 0.036).abs() < 1e-9);
        let sum = usage_summary(&c).unwrap();
        assert_eq!((sum.all_time.jobs, sum.this_month.jobs, sum.by_month.len(), sum.recent.len()), (1, 1, 1, 1));

        rename_speaker(&c, "t1", "Speaker 1", "Alice").unwrap();
        assert_eq!(search(&c, "quarterly", 10).unwrap()[0].speaker.as_deref(), Some("Alice"));

        // Re-transcribing archives the previous version (with its speaker names).
        let segs2 = vec![Segment { start: 1.0, end: 2.0, text: "Quarterly revenue is great".into(), speaker: Some("Speaker 1".into()) }];
        let v2 = CompletedVersion { provider: "gemini", model: "m2", language: Some("en"), detected_language: None, processed_duration: 1.0 };
        commit_version(&mut c, "t1", &segs2, &v2).unwrap();
        let d = get_transcript(&c, "t1").unwrap().unwrap();
        assert_eq!((d.summary.version, d.summary.model.as_str()), (2, "m2"));
        assert_eq!(d.speakers[0].display_name, "Speaker 1");
        let vs = list_versions(&c, "t1").unwrap();
        assert_eq!((vs.len(), vs[0].number, vs[0].segment_count, vs[0].model.as_str()), (1, 1, 1, "m1"));
        let old = get_version(&c, vs[0].id).unwrap().unwrap();
        assert_eq!(old.segments[0].text, "Quarterly revenue looks great");
        assert_eq!(old.speakers[0].display_name, "Alice");
        // Search only covers the current version.
        assert!(search(&c, "looks", 10).unwrap().is_empty());

        // Restore v1: v2 is archived in its place.
        restore_version(&mut c, vs[0].id).unwrap();
        let d = get_transcript(&c, "t1").unwrap().unwrap();
        assert_eq!((d.summary.version, d.segments[0].text.as_str()), (1, "Quarterly revenue looks great"));
        assert_eq!(d.speakers[0].display_name, "Alice");
        let vs = list_versions(&c, "t1").unwrap();
        assert_eq!(vs.iter().map(|v| v.number).collect::<Vec<_>>(), vec![2]);
        // A third run gets the next free number.
        commit_version(&mut c, "t1", &segs2, &v2).unwrap();
        assert_eq!(get_transcript(&c, "t1").unwrap().unwrap().summary.version, 3);
        assert_eq!(list_versions(&c, "t1").unwrap().len(), 2);

        // A failed re-run keeps the transcript usable.
        mark_failed(&c, "t1", "boom").unwrap();
        let d = get_transcript(&c, "t1").unwrap().unwrap();
        assert_eq!((d.summary.status.as_str(), d.summary.error.as_deref()), ("done", Some("boom")));

        // Usage history survives deleting the transcript.
        delete_transcript(&c, "t1").unwrap();
        let sum = usage_summary(&c).unwrap();
        assert_eq!(sum.recent[0].transcript_id, None);
        assert_eq!(sum.all_time.jobs, 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
