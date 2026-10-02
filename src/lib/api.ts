import { invoke } from "@tauri-apps/api/core";

export type ProviderKind = "openai" | "gemini";

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
}

export type SilenceMethod = "speech" | "volume";

/** Whether a provider/model returns speaker labels. */
export function diarizes(p: ProviderConfig | undefined): boolean {
  return !!p && (p.kind === "gemini" || p.model.includes("diarize"));
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

export type Status = "processing" | "done" | "error";

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

export interface UsageGroup extends UsageTotals {
  month: string;
  provider_name: string;
  model: string;
}

export interface UsageRow {
  id: number;
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
  by_month: UsageGroup[];
  recent: UsageRow[];
}

export interface TranscriptDetail extends TranscriptSummary {
  audio_path: string;
  /** Options of the most recent job (reused on retry so finished chunks aren't re-sent). */
  last_options: Partial<JobOptions> | null;
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
}

/** A stretch of a transcript that looks wrong. */
export interface ProblemRange {
  start: number;
  end: number;
  reasons: string[];
  /** "text": missing or garbled text; "speakers": only speaker labels missing. */
  kind: "text" | "speakers";
}

export type ExportFormat = "txt" | "md" | "srt" | "vtt" | "json";

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  defaultChunkMinutes: (providerId: string) => invoke<number>("default_chunk_minutes", { providerId }),

  startTranscription: (path: string, options: JobOptions) =>
    invoke<string>("start_transcription", { path, options }),
  retranscribe: (id: string, options: JobOptions) => invoke<void>("retranscribe", { id, options }),
  findProblems: (id: string) => invoke<ProblemRange[]>("find_problems", { id }),
  repairTranscript: (id: string, ranges: [number, number][], options: JobOptions) =>
    invoke<void>("repair_transcript", { id, ranges, options }),
  cancelJob: (id: string) => invoke<void>("cancel_job", { id }),

  listTranscripts: (filter: ListFilter) => invoke<TranscriptSummary[]>("list_transcripts", { filter }),
  getTranscript: (id: string) => invoke<TranscriptDetail | null>("get_transcript", { id }),
  updateTranscript: (id: string, title: string, folderId: number | null, tagIds: number[]) =>
    invoke<void>("update_transcript", { id, title, folderId, tagIds }),
  deleteTranscript: (id: string) => invoke<void>("delete_transcript", { id }),
  renameSpeaker: (id: string, label: string, name: string) => invoke<void>("rename_speaker", { id, label, name }),
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
  knownPrice: (model: string) =>
    invoke<{ per_minute: number; tokens: [number, number] | null }>("known_price", { model }),

  listFolders: () => invoke<Folder[]>("list_folders"),
  createFolder: (name: string, parentId: number | null) => invoke<Folder>("create_folder", { name, parentId }),
  renameFolder: (id: number, name: string) => invoke<void>("rename_folder", { id, name }),
  deleteFolder: (id: number) => invoke<void>("delete_folder", { id }),
  listTags: () => invoke<Tag[]>("list_tags"),
  createTag: (name: string, color: string) => invoke<Tag>("create_tag", { name, color }),
  deleteTag: (id: number) => invoke<void>("delete_tag", { id }),
};
