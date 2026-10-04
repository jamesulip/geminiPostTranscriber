//! Hash invalidation (unit) and the whole pipeline against `scripts/mock_ai_server.py` (ignored).

use super::*;
use crate::db;
use crate::providers::Segment;
use crate::settings::{ProviderConfig, ProviderKind, Settings};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

fn temp_db() -> rusqlite::Connection {
    let dir = std::env::temp_dir().join(format!("transcriber-ai-{}", uuid::Uuid::new_v4()));
    db::open(&dir.join("db.sqlite")).unwrap()
}

fn seed(c: &mut rusqlite::Connection, id: &str, title: &str) {
    db::insert_transcript(
        c,
        &db::NewTranscript {
            id,
            title,
            folder_id: None,
            audio_path: "/x.m4a",
            source_name: "meeting.m4a",
            provider: "gemini",
            model: "gemini-3.8-flash",
            language: Some("fil"),
            recorded_at: Some("2026-09-13T09:10:00+08:00"),
            tag_ids: &[],
        },
    )
    .unwrap();
    let lines = [
        "Good morning, simulan na natin ang sprint planning.",
        "Una, yung login flow ng mobile app. Kailangan natin ng OTP.",
        "Payments: GCash muna, tapos credit card sa susunod na sprint.",
        "Juan, pwede mo bang isulat ang API spec bago Friday?",
        "Sige, gagawin ko. Isasama ko yung error codes.",
        "Decision: i-ship muna ang login flow bago ang payments.",
    ];
    let segs: Vec<Segment> = (0..36)
        .map(|i| Segment { start: i as f64 * 10.0, end: i as f64 * 10.0 + 9.0, text: lines[i % lines.len()].into(), speaker: Some(format!("Speaker {}", i % 2 + 1)) })
        .collect();
    let v = db::CompletedVersion { provider: "gemini", model: "gemini-3.8-flash", language: Some("fil"), detected_language: None, processed_duration: 360.0, notes: None };
    db::commit_version(c, id, &segs, &v).unwrap();
    db::set_duration(c, id, 360.0).unwrap();
    db::set_status(c, id, "done", None).unwrap();
}

#[test]
fn content_hash_follows_text_not_names() {
    let mut c = temp_db();
    seed(&mut c, "t1", "meeting");
    let h1 = store::transcript_hash(&c, "t1").unwrap().unwrap();
    store::put_digest(&c, "t1", "summary", &h1, "m", "hello").unwrap();
    assert!(!store::digest(&c, "t1", "summary").unwrap().unwrap().stale);

    db::rename_speaker(&c, "t1", "Speaker 1", "Maria").unwrap();
    assert_eq!(store::transcript_hash(&c, "t1").unwrap().unwrap(), h1, "renaming a speaker must not stale digests");

    let seg_id: i64 = c.query_row("SELECT id FROM segments WHERE transcript_id = 't1' AND idx = 3", [], |r| r.get(0)).unwrap();
    db::update_segment(&c, seg_id, "edited text", Some("Speaker 2")).unwrap();
    let h2 = store::transcript_hash(&c, "t1").unwrap().unwrap();
    assert_ne!(h1, h2);
    assert!(store::digest(&c, "t1", "summary").unwrap().unwrap().stale);

    // Chunks keep their summaries/embeddings across a sync when their text is unchanged.
    let t = db::get_transcript(&c, "t1").unwrap().unwrap();
    let chunks = chunker::chunk(&t.segments, &Default::default());
    let stored = store::sync_chunks(&mut c, "t1", &chunks).unwrap();
    store::set_chunk_summary(&c, stored[0].id, "sum").unwrap();
    store::set_chunk_embedding(&c, stored[0].id, &[1.0, 0.0], "e").unwrap();
    let again = store::sync_chunks(&mut c, "t1", &chunks).unwrap();
    assert_eq!(again[0].summary.as_deref(), Some("sum"));
    assert_eq!(again[0].embedding.as_deref(), Some(&[1.0, 0.0][..]));

    // Restoring a version recomputes the hash too.
    let versions = db::list_versions(&c, "t1").unwrap();
    if let Some(v) = versions.first() {
        db::restore_version(&mut c, v.id).unwrap();
        assert_ne!(store::transcript_hash(&c, "t1").unwrap().unwrap(), h2);
    }
    // Deleting the transcript cascades.
    db::delete_transcript(&c, "t1").unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM ai_chunks", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

fn mock_settings(url: &str, kind: ProviderKind) -> Settings {
    let base = match kind {
        ProviderKind::Openai => format!("{url}/v1"),
        _ => url.to_string(),
    };
    let mut s = Settings::default();
    s.providers = vec![ProviderConfig {
        id: "mock".into(),
        name: "Mock".into(),
        kind,
        base_url: base,
        api_key: "test-key".into(),
        model: "whisper".into(),
        price_per_minute: None,
        price_input_per_mtok: Some(1.0),
        price_output_per_mtok: Some(2.0),
        price_cached_input_per_mtok: Some(0.1),
    }];
    s.ai_provider = "mock".into();
    s.ai_model = "mock-chat".into();
    s.ai_embedding_provider = "mock".into();
    s.ai_embedding_model = "mock-embed".into();
    s.ai_context_cache = true;
    s
}

/// `python3 ../scripts/mock_ai_server.py & MOCK_AI_URL=http://127.0.0.1:8767 cargo test ai_end_to_end -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn ai_end_to_end() {
    let url = std::env::var("MOCK_AI_URL").expect("MOCK_AI_URL");
    let mut conn = temp_db();
    seed(&mut conn, "t1", "meeting");
    let db = Mutex::new(conn);
    let http = reqwest::Client::new();
    let cancel = AtomicBool::new(false);
    let events: Mutex<Vec<AiEvent>> = Mutex::new(vec![]);
    let on_event = |e: AiEvent| events.lock().unwrap().push(e);

    for kind in [ProviderKind::Openai, ProviderKind::Gemini, ProviderKind::Anthropic] {
        let settings = mock_settings(&url, kind.clone());
        let ctx = AiCtx { http: &http, db: &db, settings: &settings, cancel: &cancel, on_event: &on_event };

        // Digest: summaries per chunk, structured digest, auto-title, embeddings.
        let out = digest::digest_transcript(&ctx, "t1", kind == ProviderKind::Openai).await.unwrap();
        assert!(!out.skipped || kind != ProviderKind::Openai);
        let kinds: Vec<&str> = out.digests.iter().map(|d| d.kind.as_str()).collect();
        assert!(kinds.contains(&"summary") && kinds.contains(&"topics"), "{kinds:?}");
        let title: String = db.lock().unwrap().query_row("SELECT title FROM transcripts WHERE id = 't1'", [], |r| r.get(0)).unwrap();
        assert_eq!(title, "Sprint planning for the mobile app");
        let chunks = store::chunks_for(&db.lock().unwrap(), "t1").unwrap();
        assert!(!chunks.is_empty() && chunks.iter().all(|c| c.summary.is_some() && c.embedding.is_some()));
        // Second run: nothing to do.
        assert!(digest::digest_transcript(&ctx, "t1", false).await.unwrap().skipped);

        // Retrieval finds the chunk by words and by meaning.
        let r = retrieval::retrieve(&ctx, &Scope::Library, "login flow OTP", 5).await.unwrap();
        assert!(!r.hits.is_empty(), "note: {:?}", r.note);
        assert!(r.hits[0].via == "both" || r.hits[0].via == "words", "{}", r.hits[0].via);

        // Ask with excerpts: streamed tokens, a valid citation kept, a bogus one dropped.
        events.lock().unwrap().clear();
        let q = format!("What was decided? ({kind:?})");
        let a = ask::ask(&ctx, ask::AskParams { thread_id: None, scope: Scope::Transcript { id: "t1".into() }, question: q.clone(), read_all: false }).await.unwrap();
        assert!(a.message.content.contains("[[t:t1:"), "{}", a.message.content);
        assert!(!a.message.content.contains("999999"));
        assert_eq!(a.message.citations.len(), 1);
        assert!(a.message.cost_usd > 0.0);
        assert!(events.lock().unwrap().iter().any(|e| matches!(e, AiEvent::Token { .. })));
        // Same question again in a new thread → memo, no tokens billed.
        let b = ask::ask(&ctx, ask::AskParams { thread_id: None, scope: Scope::Transcript { id: "t1".into() }, question: q, read_all: false }).await.unwrap();
        assert!(b.message.from_memo && b.message.cost_usd == 0.0);
        // Follow-up in the same thread carries history.
        let c2 = ask::ask(&ctx, ask::AskParams { thread_id: Some(a.thread.id.clone()), scope: Scope::Transcript { id: "t1".into() }, question: "And who owns the spec?".into(), read_all: false }).await.unwrap();
        assert_eq!(c2.thread.id, a.thread.id);
        assert_eq!(store::messages_for(&db.lock().unwrap(), &a.thread.id).unwrap().len(), 4);

        // Documents from templates come from the digests and are stored per transcript.
        let doc = docs::generate(&ctx, &Scope::Transcript { id: "t1".into() }, "mom").await.unwrap();
        assert!(doc.contains("Sources:") && doc.contains("t1"), "{doc}");
        assert!(store::digest(&db.lock().unwrap(), "t1", "doc:mom").unwrap().is_some());
        assert!(docs::generate(&ctx, &Scope::Library, "nope").await.is_err());

        // Clean-up of the current version becomes a new version; the source stays in history.
        if kind == ProviderKind::Openai {
            let before: i64 = db.lock().unwrap().query_row("SELECT version FROM transcripts WHERE id = 't1'", [], |r| r.get(0)).unwrap();
            let v = cleanup::clean_transcript(&ctx, "t1", None, cleanup::Level::Clean).await.unwrap();
            assert_eq!(v, before + 1);
            let versions = db::list_versions(&db.lock().unwrap(), "t1").unwrap();
            assert!(versions.iter().any(|x| x.number == before));
            let t = db::get_transcript(&db.lock().unwrap(), "t1").unwrap().unwrap();
            assert!(t.segments.len() >= 18 && t.summary.model.contains("clean verbatim"));
            assert!(store::digest(&db.lock().unwrap(), "t1", "summary").unwrap().unwrap().stale);
            // Put the digest back in sync for the rest of the test.
            digest::digest_transcript(&ctx, "t1", true).await.unwrap();
        }

        // Whole-transcript mode; on Gemini this goes through cachedContents.
        let full = ask::ask(&ctx, ask::AskParams { thread_id: None, scope: Scope::Transcript { id: "t1".into() }, question: format!("Summarise everything ({kind:?})"), read_all: true }).await.unwrap();
        if kind == ProviderKind::Gemini {
            assert_eq!(full.message.cached_tokens, 12345, "Gemini answer should come from the context cache");
            assert!(store::context_cache_get(&db.lock().unwrap(), "t1", "mock-chat").unwrap().is_some());
        }
    }
    let summary = db::usage_summary(&db.lock().unwrap()).unwrap();
    let kinds: Vec<&str> = summary.by_kind.iter().map(|k| k.kind.as_str()).collect();
    assert!(kinds.contains(&"ai") && kinds.contains(&"embed"), "{kinds:?}");
    assert_eq!(db::usage_for_transcript(&db.lock().unwrap(), "t1").unwrap().cost_usd, 0.0, "AI spend must not show as transcription cost");
}
