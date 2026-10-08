# Transcriber

Desktop app (Tauri 2 + Vue 3 + shadcn-vue) to drop audio files and transcribe them with
Groq / OpenAI (any OpenAI-compatible `/audio/transcriptions` API) or Gemini.

## Features
- Drag & drop or upload audio/video files.
- Cost reduction before upload: silence removal, speed-up (1–2×), Opus 24 kbps mono encoding.
- Chunking at silence boundaries; timestamps are mapped back to the **original** audio.
- Speaker diarization with Gemini `gemini-3.5-transcribe` (Interactions API) or OpenAI `gpt-4o-transcribe-diarize`.
- Library grouped by nested **folders** and **tags**.
- Transcript view: audio player, speaker timeline, click-to-seek, rename/reassign/merge speakers, edit segments.
- Fix timestamps (⋯ menu): Gemini Flash writes its own timestamps and drifts by up to a minute; this re-times every
  line against Whisper word timings (Groq, ~$0.02/hour of audio), keeping the text and speakers.
- Full-text search across all transcripts (SQLite FTS5, ⌘K), in-transcript find.
- Export TXT, Markdown, SRT, VTT, JSON.

## Setup
```bash
pnpm install
./scripts/fetch-ffmpeg.sh   # downloads a static ffmpeg sidecar into src-tauri/binaries/
pnpm tauri dev
```
Then open **Settings** and paste an API key for Groq, OpenAI, and/or Gemini.

Data (SQLite library, copied audio, `settings.json`) lives in the app data dir,
e.g. `~/Library/Application Support/ph.tigergraphics.transcriber/`. API keys are stored there in plain text.

## Now vs Overnight
- **Now**: Gemini 3.5 Transcribe (speaker labels) or Groq (text only); results in minutes, subject to rate limits.
- **Overnight (batch)**: Gemini 3.8 Flash through the Gemini Batch API — ~50% cheaper, speaker labels via prompt,
  results within 24 h. Pending batches survive app restarts; the app checks every 3 min while open.
  Test the API flow with `python3 scripts/mock_gemini_batch.py` and
  `MOCK_BATCH_URL=http://127.0.0.1:8766 cargo test batch_flow -- --ignored`.

## AI assistant
Summaries, questions with citations, meaning search, English translation, project briefs, and an MCP server
for coding agents. Set a provider under **Settings → AI assistant** (Gemini, any OpenAI-compatible chat API, or
Anthropic; the Anthropic entry only serves the assistant, it can't transcribe).

- **Summary** (Assistant panel, ⌘J): each part of a transcript is summarised once, then one structured digest is
  written: title, summary, decisions, action items, open questions, topics with times, glossary, and who-is-who
  guesses. Stored by content hash, so it's only redone when the transcript changes. On by default when a
  transcription finishes (`ai_auto_digest`).
- **Ask**: per meeting (panel) or across a folder/tag/library (sidebar → Ask, or type a question into ⌘K).
  Answers carry `[[t:<transcript id>:<seconds>]]` citations rendered as time chips that seek the player.
  Short transcripts go to the model whole; long ones and multi-meeting scopes send the digests plus the most
  relevant excerpts (FTS5 + embeddings, reciprocal rank fusion). "Read all" forces the whole transcript; with
  `ai_context_cache` on, Gemini keeps it in a `cachedContents` object for a TTL so follow-ups pay the cached rate
  (Anthropic uses `cache_control` on the transcript block automatically). Identical first questions over
  unchanged transcripts are answered from the store without a model call.
- **Meaning search**: ⌘K shows "By meaning" hits once transcripts are indexed (summaries index them; Settings has
  an "Index library" button). Embeddings (default `gemini-embedding-2`, 768 dims) are f32 BLOBs in SQLite; no vector database.
- **Glossary feedback**: terms from earlier meetings in the same folder are passed to Gemini and Whisper
  transcription prompts so names and jargon are spelled consistently.
- **English**: the transcript toolbar's "English" toggle translates once (stored as the `translation_en` digest).
- **Project brief**: Export → "Project brief (.md)" on a transcript, or "Export brief" on the Ask page for a
  folder/tag/library: requirements, decisions, constraints, open questions, next steps, each with citations.
- **Costs** land in the Usage page as `ai` / `embed` rows, priced by the provider's text rates (Settings).

### MCP server (for Claude Code and other agents)
`transcriber-mcp` speaks MCP over stdio and shares the library with the app (works while the app is closed):
```bash
cd src-tauri && cargo build --release --bin transcriber-mcp
claude mcp add transcriber -- "$PWD/../target/release/transcriber-mcp"   # or wherever cargo put it
```
Tools: `list_transcripts`, `search_transcripts`, `get_summary`, `get_transcript_text`, `ask`, `summarize_transcript`.
It reads `$TRANSCRIBER_DATA_DIR` or the app's data dir. Agents get digests and excerpts, not 50k-token transcripts.

### Testing the assistant offline
```bash
python3 scripts/mock_ai_server.py &            # OpenAI, Gemini and Anthropic shapes on :8767
MOCK_AI_URL=http://127.0.0.1:8767 cargo test ai_end_to_end -- --ignored --nocapture
```

## Long recordings (1–6 h)
- Preprocessing streams audio: ~10 s per hour of audio, ~20 MB RAM regardless of length.
- Rate limits are retried using the provider's `Retry-After` (up to 6 attempts).
- Finished chunks are cached; retrying a failed or interrupted job only re-sends unfinished chunks.
- Benchmark: `BENCH_AUDIO=/path/long.mp3 cargo test --release bench_preprocess -- --ignored --nocapture`

## Tests
```bash
cd src-tauri && cargo test
# End-to-end against a local mock provider:
python3 ../scripts/mock_openai_server.py &
MOCK_URL=http://127.0.0.1:8765 MOCK_AUDIO=/path/to/audio.mp3 cargo test end_to_end -- --ignored --nocapture
```

## Notes / limitations
- Whisper models (Groq, `whisper-1`) don't label speakers.
- Gemini transcribe chunks are diarized independently; labels can drift between chunks, so the default Gemini
  chunk is 25 min (diarized requests are capped at 30 min). Rename/reassign speakers in the transcript view.
- Live transcription is not implemented.
