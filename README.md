# Transcriber

Desktop app (Tauri 2 + Vue 3 + shadcn-vue) to drop audio files and transcribe them with
Groq / OpenAI (any OpenAI-compatible `/audio/transcriptions` API) or Gemini.

## Features
- Drag & drop or upload audio/video files.
- Cost reduction before upload: silence removal, speed-up (1–2×), Opus 24 kbps mono encoding.
- Chunking at silence boundaries; timestamps are mapped back to the **original** audio.
- Speaker diarization with Gemini `gemini-3.5-transcribe` (Interactions API) or OpenAI `gpt-4o-transcribe-diarize`.
- Library grouped by nested **folders** and **tags**.
- Transcript view: audio player, speaker timeline, click-to-seek, rename/reassign speakers, edit segments.
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
