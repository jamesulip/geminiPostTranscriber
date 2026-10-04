//! `transcriber-mcp`: an MCP server (JSON-RPC 2.0 over stdio) that lets coding agents such as
//! Claude Code use the transcript library as reference material. It shares `library.sqlite`
//! and the AI harness with the desktop app, so it reads summaries and retrieved excerpts
//! instead of whole transcripts. Works while the app is closed.
//!
//!   claude mcp add transcriber -- /path/to/transcriber-mcp
//!
//! Data dir: `$TRANSCRIBER_DATA_DIR`, else the app's data dir (macOS:
//! ~/Library/Application Support/ph.tigergraphics.transcriber).

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use transcriber_lib::ai::{self, AiCtx, Scope};
use transcriber_lib::{db, settings};

const APP_ID: &str = "ph.tigergraphics.transcriber";
const PROTOCOL: &str = "2025-06-18";

fn data_dir() -> Result<PathBuf> {
    if let Ok(d) = std::env::var("TRANSCRIBER_DATA_DIR") {
        return Ok(PathBuf::from(d));
    }
    let base = dirs::data_dir().ok_or_else(|| anyhow!("no data dir on this platform; set TRANSCRIBER_DATA_DIR"))?;
    Ok(base.join(APP_ID))
}

struct Server {
    db: Mutex<rusqlite::Connection>,
    data_dir: PathBuf,
    http: reqwest::Client,
}

fn tools() -> Value {
    let scope_props = json!({
        "transcript_id": { "type": "string", "description": "Limit to one transcript (id from list_transcripts)." },
        "folder": { "type": "string", "description": "Limit to a folder, by name (includes sub-folders)." },
        "tag": { "type": "string", "description": "Limit to a tag, by name." }
    });
    json!([
        {
            "name": "list_transcripts",
            "description": "List meeting transcripts in the library: id, title, when recorded, length, language, folder, tags, whether a summary exists. Call this first to find ids.",
            "inputSchema": { "type": "object", "properties": { "folder": scope_props["folder"], "tag": scope_props["tag"], "limit": { "type": "integer", "default": 50 } } }
        },
        {
            "name": "search_transcripts",
            "description": "Find moments in meetings by words and by meaning (works across Filipino/English). Returns excerpts with transcript id and time. Cheap: use it before reading transcripts.",
            "inputSchema": { "type": "object", "required": ["query"], "properties": { "query": { "type": "string" }, "transcript_id": scope_props["transcript_id"], "folder": scope_props["folder"], "tag": scope_props["tag"], "limit": { "type": "integer", "default": 8 } } }
        },
        {
            "name": "get_summary",
            "description": "The stored digest of one meeting: summary, decisions, action items, open questions, topics with times, glossary. A few hundred tokens instead of the whole transcript.",
            "inputSchema": { "type": "object", "required": ["transcript_id"], "properties": { "transcript_id": { "type": "string" } } }
        },
        {
            "name": "get_transcript_text",
            "description": "Verbatim transcript lines ([time] Speaker: text) for a time range of one meeting. Use start/end from search or summary topics; whole transcripts can be 50k+ tokens.",
            "inputSchema": { "type": "object", "required": ["transcript_id"], "properties": { "transcript_id": { "type": "string" }, "start": { "type": "number", "description": "seconds" }, "end": { "type": "number", "description": "seconds" }, "max_chars": { "type": "integer", "default": 20000 } } }
        },
        {
            "name": "ask",
            "description": "Ask a question about the meetings; answered from summaries and the most relevant excerpts, with [[t:transcript_id:seconds]] citations. Uses the app's configured AI provider (costs tokens).",
            "inputSchema": { "type": "object", "required": ["question"], "properties": { "question": { "type": "string" }, "transcript_id": scope_props["transcript_id"], "folder": scope_props["folder"], "tag": scope_props["tag"] } }
        },
        {
            "name": "summarize_transcript",
            "description": "Build (or refresh) the stored digest of a meeting that has none yet. Takes up to a minute or two and costs tokens; afterwards get_summary is instant.",
            "inputSchema": { "type": "object", "required": ["transcript_id"], "properties": { "transcript_id": { "type": "string" }, "force": { "type": "boolean", "default": false } } }
        }
    ])
}

impl Server {
    fn scope(&self, args: &Value) -> Result<Scope> {
        if let Some(id) = args["transcript_id"].as_str().filter(|s| !s.is_empty()) {
            return Ok(Scope::Transcript { id: id.to_string() });
        }
        let c = self.db.lock().unwrap();
        if let Some(name) = args["folder"].as_str().filter(|s| !s.is_empty()) {
            let f = db::list_folders(&c)?.into_iter().find(|f| f.name.eq_ignore_ascii_case(name)).ok_or_else(|| anyhow!("No folder named {name:?}"))?;
            return Ok(Scope::Folder { id: f.id });
        }
        if let Some(name) = args["tag"].as_str().filter(|s| !s.is_empty()) {
            let t = db::list_tags(&c)?.into_iter().find(|t| t.name.eq_ignore_ascii_case(name.trim_start_matches('#'))).ok_or_else(|| anyhow!("No tag named {name:?}"))?;
            return Ok(Scope::Tag { id: t.id });
        }
        Ok(Scope::Library)
    }

    async fn call(&self, name: &str, args: &Value) -> Result<String> {
        let settings = settings::load(&self.data_dir.join("settings.json"));
        let cancel = AtomicBool::new(false);
        let on_event = |e: ai::AiEvent| {
            if let ai::AiEvent::Progress { message, .. } = e {
                eprintln!("[transcriber-mcp] {message}");
            }
        };
        let ctx = AiCtx { http: &self.http, db: &self.db, settings: &settings, cancel: &cancel, on_event: &on_event };
        match name {
            "list_transcripts" => {
                let scope = self.scope(args)?;
                let limit = args["limit"].as_u64().unwrap_or(50) as usize;
                let c = self.db.lock().unwrap();
                let folders = db::list_folders(&c)?;
                let ts = ai::store::transcripts_in_scope(&c, &scope)?;
                let mut out = format!("{} transcript(s)\n\n", ts.len());
                for t in ts.iter().take(limit) {
                    let folder = t.folder_id.and_then(|id| folders.iter().find(|f| f.id == id)).map(|f| f.name.as_str()).unwrap_or("");
                    let tags: Vec<&str> = t.tags.iter().map(|x| x.name.as_str()).collect();
                    let has = ai::store::digest(&c, &t.id, "summary")?.map(|d| if d.stale { "summary (stale)" } else { "summary" }).unwrap_or("no summary");
                    out.push_str(&format!(
                        "- id: {} · {} · recorded {} · {} · {} · folder: {} · tags: {} · {}\n",
                        t.id, t.title, t.recorded_at.as_deref().unwrap_or("?"), ai::format_time(t.duration),
                        t.language.as_deref().unwrap_or("?"), folder, tags.join(","), has
                    ));
                }
                Ok(out)
            }
            "search_transcripts" => {
                let scope = self.scope(args)?;
                let q = args["query"].as_str().ok_or_else(|| anyhow!("query is required"))?;
                let limit = args["limit"].as_u64().unwrap_or(8) as usize;
                let r = ai::retrieval::retrieve(&ctx, &scope, q, limit).await?;
                let mut out = String::new();
                if let Some(n) = r.note {
                    out.push_str(&format!("(note: {n})\n\n"));
                }
                if r.hits.is_empty() {
                    out.push_str("No matches.");
                }
                for h in r.hits {
                    let excerpt: String = h.text.chars().take(700).collect();
                    out.push_str(&format!("## {} (id {}) at {} — found by {}\n{}\n\n", h.title, h.transcript_id, ai::format_time(h.at), h.via, excerpt));
                }
                Ok(out)
            }
            "get_summary" => {
                let id = args["transcript_id"].as_str().ok_or_else(|| anyhow!("transcript_id is required"))?;
                let c = self.db.lock().unwrap();
                let ds = ai::store::digests_for(&c, id)?;
                let title: String = c.query_row("SELECT title FROM transcripts WHERE id = ?1", [id], |r| r.get(0)).context("transcript not found")?;
                Ok(render_digests(&title, id, &ds))
            }
            "get_transcript_text" => {
                let id = args["transcript_id"].as_str().ok_or_else(|| anyhow!("transcript_id is required"))?;
                let (start, end) = (args["start"].as_f64().unwrap_or(0.0), args["end"].as_f64().unwrap_or(f64::MAX));
                let max = args["max_chars"].as_u64().unwrap_or(20_000) as usize;
                let c = self.db.lock().unwrap();
                let t = db::get_transcript(&c, id)?.ok_or_else(|| anyhow!("transcript not found"))?;
                let names: std::collections::HashMap<String, String> = t.speakers.iter().map(|s| (s.label.clone(), s.display_name.clone())).collect();
                let segs: Vec<db::SegmentRow> = t.segments.into_iter().filter(|s| s.end >= start && s.start <= end).collect();
                let text = ai::chunker::render(&segs, &names);
                let total = text.chars().count();
                let mut out: String = text.chars().take(max).collect();
                if total > max {
                    out.push_str(&format!("\n\n[truncated: {total} chars in range; narrow start/end or raise max_chars]"));
                }
                Ok(format!("# {} (id {})\n{}", t.summary.title, id, out))
            }
            "ask" => {
                let scope = self.scope(args)?;
                let q = args["question"].as_str().ok_or_else(|| anyhow!("question is required"))?;
                let a = ai::ask::ask(&ctx, ai::ask::AskParams { thread_id: None, scope, question: q.to_string(), read_all: false }).await?;
                let mut out = a.message.content.clone();
                if !a.message.citations.is_empty() {
                    out.push_str("\n\nCitations:\n");
                    for c in &a.message.citations {
                        out.push_str(&format!("- {} (id {}) at {}\n", c.title, c.transcript_id, ai::format_time(c.start)));
                    }
                }
                if let Some(n) = a.note {
                    out.push_str(&format!("\n({n})"));
                }
                Ok(out)
            }
            "summarize_transcript" => {
                let id = args["transcript_id"].as_str().ok_or_else(|| anyhow!("transcript_id is required"))?;
                let force = args["force"].as_bool().unwrap_or(false);
                let o = ai::digest::digest_transcript(&ctx, id, force).await?;
                let c = self.db.lock().unwrap();
                let title: String = c.query_row("SELECT title FROM transcripts WHERE id = ?1", [id], |r| r.get(0))?;
                Ok(format!("{}\n\n{}", if o.skipped { "Summary was already current." } else { "Summary built." }, render_digests(&title, id, &o.digests)))
            }
            other => bail!("unknown tool {other}"),
        }
    }
}

fn render_digests(title: &str, id: &str, ds: &[ai::store::Digest]) -> String {
    let get = |k: &str| ds.iter().find(|d| d.kind == k);
    let Some(summary) = get("summary") else {
        return format!("No summary stored for {title} (id {id}). Call summarize_transcript, or use search_transcripts / get_transcript_text.");
    };
    let mut out = format!("# {title} (id {id})\n");
    if summary.stale {
        out.push_str("(the transcript was edited after this summary was made)\n");
    }
    out.push_str(&format!("\n{}\n", summary.content));
    let list = |k: &str| -> Vec<Value> { get(k).and_then(|d| serde_json::from_str::<Vec<Value>>(&d.content).ok()).unwrap_or_default() };
    let secs = |v: &Value| v.as_f64().map(ai::format_time).unwrap_or_default();
    let section = |out: &mut String, heading: &str, items: Vec<String>| {
        if !items.is_empty() {
            out.push_str(&format!("\n## {heading}\n"));
            for i in items {
                out.push_str(&format!("- {i}\n"));
            }
        }
    };
    section(&mut out, "Topics", list("topics").iter().map(|t| format!("{} — {} to {}", t["title"].as_str().unwrap_or(""), secs(&t["start"]), secs(&t["end"]))).collect());
    section(&mut out, "Decisions", list("decisions").iter().map(|t| format!("{} [{}]", t["text"].as_str().unwrap_or(""), secs(&t["time"]))).collect());
    section(&mut out, "Action items", list("actions").iter().map(|t| format!("{}{}{} [{}]", t["owner"].as_str().map(|o| format!("{o}: ")).unwrap_or_default(), t["task"].as_str().unwrap_or(""), t["due"].as_str().map(|d| format!(" (due {d})")).unwrap_or_default(), secs(&t["time"]))).collect());
    section(&mut out, "Open questions", list("questions").iter().map(|t| format!("{} [{}]", t["text"].as_str().unwrap_or(""), secs(&t["time"]))).collect());
    section(&mut out, "Glossary", list("glossary").iter().map(|t| format!("{}: {}", t["term"].as_str().unwrap_or(""), t["meaning"].as_str().unwrap_or(""))).collect());
    out
}

fn respond(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

async fn handle(server: &Server, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg["method"].as_str().unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    match method {
        "initialize" => {
            let version = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
            Some(respond(id, json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "transcriber", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Meeting transcripts of the user's team (Filipino/English). Start with list_transcripts or search_transcripts; prefer get_summary over get_transcript_text; cite transcript ids and times."
            })))
        }
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(respond(id, json!({}))),
        "tools/list" => Some(respond(id, json!({ "tools": tools() }))),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Some(match server.call(name, &args).await {
                Ok(text) => respond(id, json!({ "content": [{ "type": "text", "text": text }] })),
                Err(e) => respond(id, json!({ "content": [{ "type": "text", "text": format!("Error: {e:#}") }], "isError": true })),
            })
        }
        "resources/list" => Some(respond(id, json!({ "resources": [] }))),
        "prompts/list" => Some(respond(id, json!({ "prompts": [] }))),
        _ if id.is_null() => None,
        other => Some(error(id, -32601, format!("method not found: {other}"))),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let dir = data_dir()?;
    let db_path = dir.join("library.sqlite");
    if !db_path.exists() {
        bail!("no library at {}; run the Transcriber app once or set TRANSCRIBER_DATA_DIR", db_path.display());
    }
    let server = Server { db: Mutex::new(db::open(&db_path)?), data_dir: dir, http: reqwest::Client::new() };
    eprintln!("[transcriber-mcp] serving {}", db_path.display());
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                writeln!(out, "{}", error(Value::Null, -32700, format!("parse error: {e}")))?;
                continue;
            }
        };
        if let Some(reply) = handle(&server, msg).await {
            writeln!(out, "{}", reply)?;
            out.flush()?;
        }
    }
    Ok(())
}
