# Plan: AI assistant harness, Q&A over transcripts, and next improvements

Date: 2026-10-04 · Branch: `feat/transcriber-mvp`

## 1. Where the app is

- Tauri 2 + Vue 3 desktop app. Rust backend owns SQLite (`library.sqlite`), audio preprocessing, providers (Gemini, OpenAI-compatible), batch jobs, versions, usage/cost.
- Library on this machine today: 10 transcripts, ~32 h of audio, 14 053 segments, ~1.19 M characters (7 Filipino, 3 undetected language). One 3.2 h meeting ≈ 40–60 k tokens of text; the whole library ≈ 400–600 k tokens.
- Already in place and reusable: FTS5 full-text search with triggers, `transcript_versions` (content changes are already versioned), `usage` table + Usage page, `job-progress` event plumbing for streaming UI updates, `generateContent` client for Gemini, `harness.local` for UI work without the backend.
- Uncommitted work on the branch (blocked-part fallback to 3.5 Flash, playback MP3, compare dialog, `diff.ts`) should land first so the AI work starts from a clean base.

## 2. Goals and non-goals

Goals
1. **AI harness**: one shared backend module every AI feature calls, so adding a feature means writing a prompt and a UI, not another HTTP client.
2. **Ask**: ask a question about one transcript, a folder/tag, or the whole library; answers cite `[transcript, mm:ss]` and clicking a citation seeks the player.
3. **Summaries**: per-transcript digest (summary, decisions, action items, open questions, topics with timestamps), generated once and stored.
4. **Semantic search**: ⌘K finds by meaning as well as by words, across languages (Taglish ↔ English).
5. **Reference for software dev**: get meeting knowledge into the dev workflow: requirement/brief exports with citations, and a local MCP server so Claude Code (or any MCP client) can query the library directly.
6. **Never re-read the whole transcript per question.** Three cache tiers (below) make the common question cost a few thousand tokens, not 50 k.

Non-goals (for now): live transcription, cloud sync, multi-user, fine-tuning.

## 3. The AI harness

### 3.1 Backend module layout (Rust)

```
src-tauri/src/ai/
  mod.rs        // public API: ask(), digest(), embed(), search_semantic(), threads
  chat.rs       // ChatProvider trait + Gemini (generateContent / streamGenerateContent)
                //   + OpenAI-compatible (/chat/completions) implementations
  embed.rs      // EmbedProvider trait + Gemini embeddings (+ OpenAI-compatible)
  chunker.rs    // segments → topic chunks (time-bounded, speaker-aware)
  digest.rs     // map-reduce summarisation per chunk → per transcript
  prompts.rs    // all prompt templates in one place, versioned (PROMPT_VERSION)
  context.rs    // builds the prompt: digests + retrieved chunks, stable prefix ordering
  cache.rs      // content hashing, Gemini cachedContents handles, memoised answers
  mcp.rs        // (phase 4) stdio/HTTP MCP server exposing the same functions
```

Design rules
- Every call goes through `ai::chat::complete(req)` which returns text + `TokenUsage` (reuse `providers::TokenUsage` / `extract_usage`), and every call writes a `usage` row with a new `kind = 'ai'` column so the Usage page shows Q&A cost next to transcription cost.
- Streaming: `complete_stream` emits `ai-token { thread_id, delta }` Tauri events, same pattern as `job-progress`. Frontend `useAi` store listens.
- Cancellation: `AtomicBool` like `Request.cancel` in `providers/mod.rs`.
- Rate limits / `Retry-After`: reuse the retry loop in `providers::transcribe` (factor it out into `providers::retry`).
- Settings additions: `ai_provider` (provider id), `ai_model`, `ai_embedding_model`, `ai_auto_digest: bool` (digest each transcript when it finishes), `ai_context_cache: bool`, prices for text models in the `known_token_price` table (the current table holds audio-input rates; text rates differ).

### 3.2 Data model (SQLite migration)

```sql
-- Stable identity of what the AI has seen. Changes when segments/speakers change.
ALTER TABLE transcripts ADD COLUMN content_hash TEXT;        -- sha256 over (idx,start,end,speaker,text) + speaker names

CREATE TABLE ai_chunks (                                      -- retrieval unit, ~6–10 min / 1.5–3 k tokens
  id INTEGER PRIMARY KEY,
  transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL,
  start REAL NOT NULL, end REAL NOT NULL,
  first_segment INTEGER NOT NULL, last_segment INTEGER NOT NULL,
  chunk_hash TEXT NOT NULL,                                   -- so an edit re-embeds one chunk, not the transcript
  text TEXT NOT NULL,                                         -- "[12:34] Juan: ..." lines, what the model sees
  summary TEXT,                                               -- 2–4 sentence chunk digest (map step)
  embedding BLOB,                                             -- f32 LE vector; NULL until embedded
  embed_model TEXT,
  UNIQUE (transcript_id, idx)
);

CREATE TABLE ai_digests (                                     -- per-transcript derived artifacts
  transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,                                         -- 'summary' | 'decisions' | 'actions' | 'topics' | 'glossary' | 'translation_en' | 'brief'
  content_hash TEXT NOT NULL,                                 -- transcript hash it was made from
  prompt_version INTEGER NOT NULL,
  model TEXT NOT NULL,
  content TEXT NOT NULL,                                      -- markdown or JSON depending on kind
  created_at TEXT NOT NULL,
  PRIMARY KEY (transcript_id, kind)
);

CREATE TABLE ai_threads (
  id TEXT PRIMARY KEY, title TEXT NOT NULL,
  scope_json TEXT NOT NULL,                                   -- {kind:'transcript'|'folder'|'tag'|'library', id}
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE ai_messages (
  id INTEGER PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES ai_threads(id) ON DELETE CASCADE,
  role TEXT NOT NULL, content TEXT NOT NULL,
  citations_json TEXT,                                        -- [{transcript_id, start, end, segment_id}]
  input_tokens INTEGER, cached_tokens INTEGER, output_tokens INTEGER, cost_usd REAL,
  created_at TEXT NOT NULL
);

CREATE TABLE ai_context_cache (                               -- Gemini explicit cache handles (tier 3)
  transcript_id TEXT NOT NULL, content_hash TEXT NOT NULL, model TEXT NOT NULL,
  cache_name TEXT NOT NULL, expires_at TEXT NOT NULL, tokens INTEGER NOT NULL,
  PRIMARY KEY (transcript_id, model)
);

CREATE TABLE ai_answer_memo (                                 -- exact repeat questions cost nothing
  key TEXT PRIMARY KEY,                                       -- sha256(scope + content hashes + normalised question + prompt_version + model)
  message_id INTEGER NOT NULL REFERENCES ai_messages(id) ON DELETE CASCADE
);
```

`content_hash` is recomputed in `commit_version*`, `update_segment`, `rename_speaker`, `restore_version`. Anything keyed by an old hash is stale; stale rows are kept until replaced so the UI can show "summary is from before your edits" instead of nothing.

### 3.3 Three cache tiers (why the LLM does not re-read transcripts)

| Tier | What is cached | Where | Invalidated by | Saves |
|---|---|---|---|---|
| 1 Derived | chunk summaries, transcript digests, EN translation | `ai_chunks.summary`, `ai_digests` | `content_hash` / `chunk_hash` change, `PROMPT_VERSION` bump | Re-summarising; library-wide questions read ~10 digests (≈3 k tokens) instead of 500 k |
| 2 Retrieval | chunk embeddings + FTS5 | `ai_chunks.embedding`, `segments_fts` | `chunk_hash` change or embedding model change | A question sends the digest + top 6–10 chunks (≈8–15 k tokens) instead of the whole transcript |
| 3 Provider | the full transcript as a Gemini `cachedContents` object with a TTL | `ai_context_cache` | TTL expiry, hash change, model change | "Deep dive" mode: many questions on one 3 h meeting at the cached-input rate, no re-upload |

Plus `ai_answer_memo` for exact repeats, and prompt ordering that keeps the stable part first (system → digests → chunks → history → question) so Gemini's implicit prefix caching also hits.

Routing: a question with scope = one transcript shorter than ~20 k tokens is answered with the whole transcript in context (tier 3 if enabled). Longer transcripts and multi-transcript scopes use tier 1 + 2. The user can force "read everything" per thread.

### 3.4 Chunker

- Walk segments in order; cut a chunk when it reaches ~2 k tokens (estimate 4 chars/token, Filipino ≈ 3) **and** there is a speaker change or a gap > 2 s; hard cap ~3.5 k tokens. Overlap one segment on each side.
- Render each chunk as `[hh:mm:ss] Speaker name: text` using `speakers.display_name` so renames flow into the model's view without re-embedding (speaker names are excluded from `chunk_hash`; only text/timing count).
- Existing `breaks.ts` logic (pauses) can inform topic boundaries; port the thresholds, do not duplicate the UI code.

### 3.5 Retrieval

- Hybrid: FTS5 BM25 on `segments_fts` (exact names, numbers, English terms) + cosine over `ai_chunks.embedding` (meaning, cross-language). Brute-force cosine in Rust over a few thousand vectors is < 5 ms; no vector DB.
- Reciprocal rank fusion, then expand each hit to its chunk, de-duplicate, cap total tokens. Return `(chunk, score, why)` so the UI can show "found via words / via meaning".
- Embedding model must be multilingual (Taglish). Use the current Gemini embedding model via the existing Gemini key; make it configurable. Verify the model name and dimension in the Gemini docs when implementing; do not hard-code one from memory.

### 3.6 Grounded answers and citations

- The model is asked to answer in markdown and to mark every claim with `[[t:<transcript_id>:<start_seconds>]]` tokens taken only from the provided chunks' timestamps. Backend validates each token against real chunk ranges, drops invalid ones, and returns `citations_json`.
- Frontend renders tokens as chips `Sept 13 · 1:02:33`; click → `router.push({name:'transcript', params:{id}, query:{t}})` (same as search hits today).
- If retrieval finds nothing relevant above a threshold, the harness answers "not in these transcripts" rather than letting the model guess.

### 3.7 Developer harness (testing)

- `scripts/mock_ai_server.py`: OpenAI-compatible `/chat/completions` (+ streaming) and `/embeddings` returning deterministic text and hashed pseudo-embeddings, so `cargo test ai_end_to_end -- --ignored` runs the whole pipeline offline (same approach as `mock_openai_server.py` / `mock_gemini_batch.py`).
- Unit tests: chunker boundaries, hash invalidation on edit/rename/restore, citation validation, RRF merge, context budget trimming, memo hits.
- `harness.local`: add canned digests/answers so the Ask panel and Summary panel can be developed without API calls.

## 4. User-facing features

### F1. Summary panel (per transcript)
Right-side tab in `TranscriptView`: Summary · Decisions · Action items · Open questions · Topics (timestamped, clickable). "Generate"/"Regenerate" button, staleness badge when `content_hash` moved. Optional auto-generate when a job finishes (`ai_auto_digest`). Export summary as part of MD/TXT export.

### F2. Ask
- Entry points: "Ask" button in the transcript header (scope = this transcript); "Ask" in the sidebar (scope = current folder/tag/all); ⌘K → type a question → "Ask AI" row.
- Chat UI with streaming, citations, suggested questions ("What was decided about X?", "List action items for Juan", "What did we say about the login flow?"), cost per answer shown small under each message, thread list per scope.
- Scope chip editable mid-thread; "Read whole transcript" toggle per thread (tier 3).

### F3. Semantic search
`SearchDialog`: results from hybrid retrieval; group by transcript; snippet from chunk; label "meaning" vs "words". Falls back to FTS-only when embeddings are missing or no key.

### F4. Transcripts as a software-dev reference
- **Brief export**: "Export → Project brief (MD)" for a transcript, folder, or tag: requirements, decisions, constraints, open questions, each line with citations. A dev can paste it into a ticket or a `docs/` folder. Stored as `ai_digests.kind='brief'`.
- **MCP server** (`transcriber-mcp`, stdio, started from the app or as a separate binary sharing `library.sqlite`): tools `search_transcripts(query, scope)`, `get_summary(transcript_id)`, `get_segments(transcript_id, start, end)`, `ask(question, scope)`, `list_transcripts(filter)`. Claude Code then references meetings while coding without the user copying text around, and because it reads digests and chunks it never pulls 500 k tokens. Register with `claude mcp add transcriber -- <path>`.
- **Copy as context**: button on a transcript/selection that copies "[Sept 13 · 1:02:33] Juan: ..." lines for pasting into a prompt.

### F5. Glossary that improves transcription
`ai_digests.kind='glossary'`: project names, people, products, Taglish terms per folder. Feed the folder's glossary into `transcription_request` ("Terms that may appear: …") and the batch prompt. This should visibly cut proper-noun errors in follow-up meetings in the same folder.

### F6. English translation (cached)
`kind='translation_en'` per chunk and per transcript, generated on demand, shown as a toggle in the transcript view and used by the brief export. Seven of ten transcripts are Filipino; devs who read English get a one-click view, paid for once.

## 5. Non-AI improvements worth scheduling

1. Land and commit the current uncommitted work (fallback chain, playback MP3, Compare dialog).
2. API keys in the macOS keychain instead of `settings.json` plain text (the README flags this).
3. Auto-title transcripts from the first digest when the title is still the file name.
4. Text-model prices in `known_token_price` and a "cached input" rate so AI cost estimates are right.
5. `usage.kind` column and Usage page breakdown (transcription vs AI vs embeddings).
6. Speaker-name suggestions from context (F1 output "Speaker 2 is probably Maria (addressed by name at 12:04)").
7. Export whole folders; export summaries with transcripts.
8. Keyboard: `⌘J` opens Ask, `⌘K` stays search.

## 6. Phases

| Phase | Scope | Ships when |
|---|---|---|
| 0 Harness | `ai/` module, chat + embed providers, settings, `content_hash`, migrations, `usage.kind`, mock AI server, tests | `cargo test` green against the mock; "Hello" round-trip from Settings page |
| 1 Summaries | chunker, map-reduce digest, Summary panel, staleness, auto-digest option, auto-title | Every existing transcript has a digest; cost shown on Usage |
| 2 Ask one transcript | threads/messages, streaming, citations → seek, memo, Gemini context cache for long meetings | Ask works on the 3.2 h Sept 13 meeting with clickable citations |
| 3 Library-wide | embeddings, hybrid retrieval, scope = folder/tag/library, semantic ⌘K | Cross-meeting question answered with citations from 2+ transcripts |
| 4 Dev reference | brief export, MCP server, copy-as-context | Claude Code answers a question about a meeting via MCP |
| 5 Feedback loops | glossary into transcription prompts, EN translation view, speaker suggestions | Measured drop in proper-noun fixes on a new meeting in the same folder |

Each phase is one PR; phases 1–2 are the ones that change daily use, so they go first after the harness.

## 7. Cost picture (order of magnitude, verify text rates before shipping)

| Operation | Tokens | Notes |
|---|---|---|
| Digest whole current library once | ~0.5 M in, ~30 k out | One-off; Flash-class text rates → well under a dollar |
| Embed whole library once | ~0.5 M | Embedding rates are a fraction of chat rates |
| One question, tier 1+2 | 8–15 k in, ~0.5 k out | The everyday case |
| One question, whole 3 h transcript, no cache | 50–60 k in | Avoided by tiers 1–3 |
| Same, with Gemini context cache | cached-input rate + small storage fee per hour of TTL | Only worth it for a burst of questions; TTL default 30–60 min |

## 8. Decisions taken in this plan (change if you disagree)

- **Default chat/embedding provider: Gemini**, because the key already exists, the transcripts are already sent there, and it offers explicit context caching. The OpenAI-compatible path (Groq chat models) is implemented in the same trait for speed/cost comparisons. Anthropic can be added as a third `ChatProvider` later without touching features.
- **No vector database.** Embeddings live in SQLite as BLOBs; brute-force cosine is enough for tens of thousands of chunks.
- **Derived artifacts are stored, not recomputed.** Every AI output that is not a chat reply is a row keyed by content hash and prompt version.
- **MCP server over a custom CLI** for dev-tool integration; it is the format Claude Code and other agents already speak.

## 9. Open questions for you

1. Should answers default to English even when the meeting is Filipino, or follow the question's language? (Plan assumes: follow the question.)
2. Auto-digest every finished transcript (costs cents, gives auto-titles and glossary) or on demand only? (Plan assumes: on, with a setting.)
3. MCP server inside the app process (only works while the app runs) or a separate small binary? (Plan assumes: separate binary in `src-tauri/src/bin/`, sharing `ai/` code.)
