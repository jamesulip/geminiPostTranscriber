import { invoke } from "@tauri-apps/api/core";

export type ProviderKind = "openai" | "gemini" | "anthropic";

export interface ProviderConfig {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  api_key: string;
  model: string;
  /** USD per audio minute; null = built-in estimate. */
  price_per_minute: number | null;
  /** USD per 1M tokens, used when the provider reports tokens; null = built-in. */
  price_input_per_mtok: number | null;
  price_output_per_mtok: number | null;
  /** USD per 1M prompt tokens served from the provider's cache (AI assistant); null = built-in. */
  price_cached_input_per_mtok: number | null;
}

/** Text prices for the AI assistant (USD per 1M tokens). */
export interface TextPrice {
  input: number;
  output: number;
  cached_input: number;
}

export type SilenceMethod = "speech" | "volume";

/** Whether a provider/model returns speaker labels. */
export function diarizes(p: ProviderConfig | undefined): boolean {
  return !!p && (p.kind === "gemini" || p.model.includes("diarize"));
}

/** Whether a provider can transcribe audio (Anthropic is text-only, for the assistant). */
export function transcribes(p: ProviderConfig | undefined): boolean {
  return !!p && p.kind !== "anthropic";
}

export const PROVIDER_KIND_LABEL: Record<ProviderKind, string> = {
  openai: "OpenAI-compatible",
  gemini: "Gemini",
  anthropic: "Anthropic",
};

/** Default chat model when a provider is picked for the assistant. */
export function defaultChatModel(kind: ProviderKind): string {
  return kind === "gemini" ? "gemini-3.8-flash" : kind === "anthropic" ? "claude-opus-5-5" : "gpt-4o-mini";
}

export interface Settings {
  settings_version: number;
  providers: ProviderConfig[];
  /** Cheapest provider, used when speaker labels aren't needed. */
  default_provider: string;
  /** Provider used when speaker labels are requested. */
  diarize_provider: string;
  language: string;
  speed: number;
  diarize_speed: number;
  /** Model for "Overnight (batch)" jobs. */
  batch_model: string;
  batch_chunk_minutes: number;
  /** Re-do batch parts Google blocked/left empty right away with the speaker-label provider. */
  batch_retry_failed: boolean;
  /** AI assistant: provider id and chat model for questions and summaries. */
  ai_provider: string;
  ai_model: string;
  /** Embeddings (semantic search). Anthropic has none. */
  ai_embedding_provider: string;
  ai_embedding_model: string;
  /** Summarise each transcript when it finishes. */
  ai_auto_digest: boolean;
  /** Keep a whole transcript in the provider's prompt cache while asking several questions. */
  ai_context_cache: boolean;
  ai_context_cache_ttl_minutes: number;
  /** Custom document templates; an id matching a built-in overrides it. */
  ai_templates: AiTemplate[];
  remove_silence: boolean;
  silence_method: SilenceMethod;
  vad_threshold: number;
  silence_threshold_db: number;
  silence_min_duration: number;
  chunk_minutes: number;
  gemini_chunk_minutes: number;
}

export interface Folder {
  id: number;
  name: string;
  parent_id: number | null;
}

export interface Tag {
  id: number;
  name: string;
  color: string;
}

/** "queued": sent to Google as an overnight batch, waiting for results. */
export type Status = "processing" | "queued" | "done" | "error";

export interface TranscriptSummary {
  id: string;
  title: string;
  folder_id: number | null;
  source_name: string;
  duration: number;
  processed_duration: number;
  provider: string;
  model: string;
  status: Status;
  error: string | null;
  created_at: string;
  tags: Tag[];
  preview: string | null;
  /** When the audio was recorded (file metadata or modification time). */
  recorded_at: string | null;
  /** Requested language, else the one detected by the provider. */
  language: string | null;
  /** Number of the current transcription version. */
  version: number;
  /** When the current version was produced. */
  transcribed_at: string | null;
  /** Parts of the current version that may be incomplete (one per line). */
  notes: string | null;
  /** Marked by the user for attention. */
  flagged: boolean;
  /** Set while the transcript is in the trash. */
  trashed_at: string | null;
  /** Identity of the current text; AI summaries compare against it to show staleness. */
  content_hash: string | null;
}

/** An archived (non-current) transcription. */
export interface VersionInfo {
  id: number;
  number: number;
  provider: string;
  model: string;
  language: string | null;
  processed_duration: number;
  transcribed_at: string;
  segment_count: number;
}

export interface VersionDetail extends VersionInfo {
  segments: SegmentRow[];
  speakers: Speaker[];
}

export interface SegmentRow {
  id: number;
  idx: number;
  start: number;
  end: number;
  speaker: string | null;
  text: string;
}

export interface Speaker {
  label: string;
  display_name: string;
}

export interface UsageTotals {
  jobs: number;
  original_seconds: number;
  sent_seconds: number;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  saved_usd: number;
}

export type UsageKind = "transcribe" | "ai" | "embed";

export interface UsageGroup extends UsageTotals {
  month: string;
  kind: UsageKind;
  provider_name: string;
  model: string;
}

export interface UsageRow {
  id: number;
  kind: UsageKind;
  transcript_id: string | null;
  title: string;
  provider_name: string;
  model: string;
  status: "done" | "error";
  original_seconds: number;
  sent_seconds: number;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  saved_usd: number;
  created_at: string;
}

export interface UsageSummary {
  all_time: UsageTotals;
  this_month: UsageTotals;
  by_kind: ({ kind: UsageKind } & UsageTotals)[];
  by_month: UsageGroup[];
  recent: UsageRow[];
}

export interface TranscriptDetail extends TranscriptSummary {
  audio_path: string;
  /** Options of the most recent job (reused on retry so finished chunks aren't re-sent). */
  last_options: Partial<JobOptions> | null;
  /** Chunks finished so far by the current (running or failed) job. */
  partial: { done: number; total: number; segments: Omit<SegmentRow, "id" | "idx">[] } | null;
  /** Pending overnight batch while status is "queued". */
  batch: { submitted_at: string; chunks: number | null; model: string } | null;
  segments: SegmentRow[];
  speakers: Speaker[];
  usage: UsageTotals;
}

export interface SearchHit {
  transcript_id: string;
  transcript_title: string;
  segment_id: number;
  start: number;
  speaker: string | null;
  snippet: string;
}

export interface JobOptions {
  provider_id: string;
  model: string | null;
  language: string | null;
  speed: number;
  remove_silence: boolean;
  silence_threshold_db: number;
  silence_min_duration: number;
  chunk_minutes: number;
  silence_method: SilenceMethod;
  vad_threshold: number;
  /** Send through the Gemini Batch API (cheaper, results within 24 h). */
  batch: boolean;
  title: string | null;
  folder_id: number | null;
  tag_ids: number[];
}

export interface Progress {
  id: string;
  stage: string;
  progress: number;
  message: string;
  done: boolean;
  error: string | null;
}

export interface ListFilter {
  folder_id: number | null;
  tag_id: number | null;
  recursive: boolean;
  /** List the trash instead of the library. */
  trashed: boolean;
}

/** A stretch of a transcript that looks wrong. */
export interface ProblemRange {
  start: number;
  end: number;
  reasons: string[];
  /** "text": missing or garbled text; "speakers": only speaker labels missing. */
  kind: "text" | "speakers";
}

/** One overnight batch (Google's live status merged with the app's records). */
export interface BatchView {
  name: string;
  title: string;
  transcript_id: string | null;
  model: string;
  state: "PENDING" | "RUNNING" | "SUCCEEDED" | "FAILED" | "CANCELLED" | "EXPIRED" | string;
  created: string | null;
  ended: string | null;
  parts: number | null;
  /** Parts the app found blocked or empty (Google still reports those as succeeded). */
  blocked: number | null;
  retried: boolean;
  cost_usd: number | null;
  outcome: string | null;
}

/** Loudness over time: one value (0..255, -60..0 dBFS) per `step` seconds. */
export interface AudioLevels {
  step: number;
  levels: number[];
}

export type ExportFormat = "txt" | "md" | "srt" | "vtt" | "json";

// AI assistant ---------------------------------------------------------------------

/** What a question or search is about. */
export type Scope = { kind: "transcript"; id: string } | { kind: "folder"; id: number } | { kind: "tag"; id: number } | { kind: "library" };

export type DigestKind = "summary" | "decisions" | "actions" | "questions" | "topics" | "glossary" | "speakers" | "translation_en" | "brief";

export interface Digest {
  kind: DigestKind | string;
  content_hash: string;
  prompt_version: number;
  model: string;
  /** Markdown (summary, brief, translation) or a JSON array (lists). */
  content: string;
  created_at: string;
  /** The transcript changed since this was made. */
  stale: boolean;
}

export interface AiThread {
  id: string;
  title: string;
  scope: Scope;
  created_at: string;
  updated_at: string;
}

export interface Citation {
  transcript_id: string;
  title: string;
  start: number;
}

export interface AiMessage {
  id: number;
  thread_id: string;
  role: "user" | "assistant";
  /** Markdown; answers carry `[[t:<transcript id>:<seconds>]]` citation tokens. */
  content: string;
  citations: Citation[];
  model: string | null;
  input_tokens: number;
  cached_tokens: number;
  output_tokens: number;
  cost_usd: number;
  created_at: string;
  from_memo: boolean;
}

export interface AiHit {
  chunk_id: number;
  transcript_id: string;
  title: string;
  idx: number;
  start: number;
  end: number;
  text: string;
  score: number;
  via: "words" | "meaning" | "both";
  /** Best time to seek to. */
  at: number;
}

export interface Retrieval {
  hits: AiHit[];
  note: string | null;
}

export type AiEvent =
  | { type: "token"; thread_id: string; delta: string }
  | { type: "progress"; key: string; message: string; progress: number; done: boolean; error: string | null };

export interface AiAnswerEvent {
  thread_id: string;
  thread?: AiThread;
  message?: AiMessage;
  note?: string | null;
  error: string | null;
}

export interface AiTemplate {
  id: string;
  name: string;
  description: string;
  /** What to produce; the app adds citation rules and the meeting material. */
  prompt: string;
  builtin?: boolean;
}

/** Non-verbatim copy: "clean" removes fillers/false starts; "smart" rewrites turns into clear sentences. */
export type CleanupLevel = "clean" | "smart";

export interface AskParams {
  thread_id: string | null;
  scope: Scope;
  question: string;
  /** Put the whole transcript in context even when it's long. */
  read_all: boolean;
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  defaultChunkMinutes: (providerId: string) => invoke<number>("default_chunk_minutes", { providerId }),

  startTranscription: (path: string, options: JobOptions) =>
    invoke<string>("start_transcription", { path, options }),
  retranscribe: (id: string, options: JobOptions) => invoke<void>("retranscribe", { id, options }),
  findProblems: (id: string) => invoke<ProblemRange[]>("find_problems", { id }),
  /** MP3 copy of the audio (made on first use) for recordings the player can't open directly. */
  playbackAudio: (id: string) => invoke<string>("playback_audio", { id }),
  audioLevels: (id: string) => invoke<AudioLevels>("audio_levels", { id }),
  /** Find who spoke when with the local diarizer; progress arrives as a "diarize" job. */
  diarize: (id: string, numSpeakers: number | null) => invoke<void>("diarize_transcript", { id, numSpeakers }),
  diarizationInfo: () => invoke<{ models_ready: boolean; realtime_factor: number }>("diarization_info"),
  speechInGaps: (id: string, gaps: [number, number][]) => invoke<number[]>("speech_in_gaps", { id, gaps }),
  repairTranscript: (id: string, ranges: [number, number][], options: JobOptions) =>
    invoke<void>("repair_transcript", { id, ranges, options }),
  /** Re-time the lines against a Whisper provider's word timings; progress arrives as an "align" job. */
  fixTimestamps: (id: string, options: JobOptions) => invoke<void>("fix_timestamps", { id, options }),
  cancelJob: (id: string) => invoke<void>("cancel_job", { id }),
  jobProgress: () => invoke<Progress[]>("job_progress"),
  checkBatches: () => invoke<void>("check_batches"),
  listBatches: () => invoke<{ batches: BatchView[]; warning: string | null }>("list_batches"),

  listTranscripts: (filter: ListFilter) => invoke<TranscriptSummary[]>("list_transcripts", { filter }),
  getTranscript: (id: string) => invoke<TranscriptDetail | null>("get_transcript", { id }),
  updateTranscript: (id: string, title: string, folderId: number | null, tagIds: number[]) =>
    invoke<void>("update_transcript", { id, title, folderId, tagIds }),
  deleteTranscript: (id: string) => invoke<void>("delete_transcript", { id }),
  trashTranscript: (id: string) => invoke<void>("trash_transcript", { id }),
  restoreTranscript: (id: string) => invoke<void>("restore_transcript", { id }),
  emptyTrash: () => invoke<void>("empty_trash"),
  setFlagged: (id: string, flagged: boolean) => invoke<void>("set_flagged", { id, flagged }),
  renameSpeaker: (id: string, label: string, name: string) => invoke<void>("rename_speaker", { id, label, name }),
  /** Relabel the `from` speakers as `into` (optionally renaming it); returns the archived version's id. */
  mergeSpeakers: (id: string, from: string[], into: string, name: string | null) =>
    invoke<number>("merge_speakers", { id, from, into, name }),
  updateSegment: (segmentId: number, text: string, speaker: string | null) =>
    invoke<void>("update_segment", { segmentId, text, speaker }),
  search: (query: string) => invoke<SearchHit[]>("search", { query }),
  exportTranscript: (id: string, format: ExportFormat, path: string, versionId: number | null = null) =>
    invoke<void>("export_transcript", { id, format, path, versionId }),
  listVersions: (id: string) => invoke<VersionInfo[]>("list_versions", { id }),
  getVersion: (versionId: number) => invoke<VersionDetail | null>("get_version", { versionId }),
  restoreVersion: (versionId: number) => invoke<void>("restore_version", { versionId }),
  deleteVersion: (versionId: number) => invoke<void>("delete_version", { versionId }),

  usageSummary: () => invoke<UsageSummary>("usage_summary"),
  // AI assistant
  aiDigest: (id: string, force = false) => invoke<void>("ai_digest", { id, force }),
  aiGetDigests: (id: string) => invoke<Digest[]>("ai_get_digests", { id }),
  aiRunning: () => invoke<string[]>("ai_running"),
  aiCancel: (key: string) => invoke<void>("ai_cancel", { key }),
  /** Returns the thread id; tokens arrive on `ai-event`, the final message on `ai-answer`. */
  aiAsk: (params: AskParams) => invoke<string>("ai_ask", { params }),
  aiListThreads: (scope: Scope | null = null) => invoke<AiThread[]>("ai_list_threads", { scope }),
  aiGetThread: (id: string) => invoke<{ thread: AiThread; messages: AiMessage[] } | null>("ai_get_thread", { id }),
  aiDeleteThread: (id: string) => invoke<void>("ai_delete_thread", { id }),
  aiSearch: (query: string, scope: Scope, limit = 20) => invoke<Retrieval>("ai_search", { query, scope, limit }),
  aiEmbedLibrary: () => invoke<void>("ai_embed_library"),
  aiTestConnection: (providerId: string, model: string) => invoke<string>("ai_test_connection", { providerId, model }),
  aiTemplates: () => invoke<AiTemplate[]>("ai_templates"),
  /** Markdown document with citations from a template, for a scope (takes ~30 s). */
  aiDocument: (scope: Scope, templateId: string) => invoke<string>("ai_document", { scope, templateId }),
  /** Translate to English in the background; result arrives as the `translation_en` digest. */
  aiTranslate: (id: string, force = false) => invoke<void>("ai_translate", { id, force }),
  saveText: (path: string, text: string) => invoke<void>("save_text", { path, text }),
  /** Clean up a version in the background (key `cleanup:<id>`); the result becomes the new current version. */
  aiCleanup: (id: string, versionId: number | null, level: CleanupLevel) => invoke<void>("ai_cleanup", { id, versionId, level }),

  knownPrice: (model: string) =>
    invoke<{ per_minute: number; tokens: [number, number] | null; text: TextPrice | null }>("known_price", { model }),

  listFolders: () => invoke<Folder[]>("list_folders"),
  createFolder: (name: string, parentId: number | null) => invoke<Folder>("create_folder", { name, parentId }),
  renameFolder: (id: number, name: string) => invoke<void>("rename_folder", { id, name }),
  deleteFolder: (id: number) => invoke<void>("delete_folder", { id }),
  listTags: () => invoke<Tag[]>("list_tags"),
  createTag: (name: string, color: string) => invoke<Tag>("create_tag", { name, color }),
  updateTag: (id: number, name: string, color: string) => invoke<void>("update_tag", { id, name, color }),
  deleteTag: (id: number) => invoke<void>("delete_tag", { id }),
};
