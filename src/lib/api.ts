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
}

export interface Settings {
  providers: ProviderConfig[];
  default_provider: string;
  language: string;
  speed: number;
  remove_silence: boolean;
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

export type ExportFormat = "txt" | "md" | "srt" | "vtt" | "json";

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  defaultChunkMinutes: (providerId: string) => invoke<number>("default_chunk_minutes", { providerId }),

  startTranscription: (path: string, options: JobOptions) =>
    invoke<string>("start_transcription", { path, options }),
  retranscribe: (id: string, options: JobOptions) => invoke<void>("retranscribe", { id, options }),
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
  knownPrice: (model: string) => invoke<number>("known_price", { model }),

  listFolders: () => invoke<Folder[]>("list_folders"),
  createFolder: (name: string, parentId: number | null) => invoke<Folder>("create_folder", { name, parentId }),
  renameFolder: (id: number, name: string) => invoke<void>("rename_folder", { id, name }),
  deleteFolder: (id: number) => invoke<void>("delete_folder", { id }),
  listTags: () => invoke<Tag[]>("list_tags"),
  createTag: (name: string, color: string) => invoke<Tag>("create_tag", { name, color }),
  deleteTag: (id: number) => invoke<void>("delete_tag", { id }),
};
