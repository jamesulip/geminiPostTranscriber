<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { onClickOutside, refDebounced, useEventListener } from "@vueuse/core";
import { useRoute, useRouter } from "vue-router";
import { convertFileSrc } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { toast } from "vue-sonner";
import {
  AlertCircle, AlertTriangle, ArrowLeft, Check, Download, GitCompare, History, Loader2, LocateFixed, MoreHorizontal, Pause,
  Flag, Play, Moon, RefreshCw, RotateCcw, Search, Shrink, Trash2, UserSearch, Users, Wrench,
} from "@lucide/vue";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import FolderSelect from "@/components/FolderSelect.vue";
import TagPicker from "@/components/TagPicker.vue";
import SpeakerTimeline from "@/components/SpeakerTimeline.vue";
import LevelMeter from "@/components/LevelMeter.vue";
import SpeakerIdDialog from "@/components/SpeakerIdDialog.vue";
import DiarizeDialog from "@/components/DiarizeDialog.vue";
import TranscriptMeta from "@/components/TranscriptMeta.vue";
import TranscriptList from "@/components/TranscriptList.vue";
import AiPanel from "@/components/AiPanel.vue";
import { ClipboardCopy, FileText, Languages, Sparkles } from "@lucide/vue";
import { useAi } from "@/stores/ai";
import { useLocalStorage } from "@vueuse/core";
import FixProblemsDialog from "@/components/FixProblemsDialog.vue";
import CompareDialog from "@/components/CompareDialog.vue";
import OptionsForm from "@/components/OptionsForm.vue";
import PromptDialog from "@/components/PromptDialog.vue";
import {
  api, type AudioLevels, type ExportFormat, type JobOptions, type SegmentRow, type TranscriptDetail, type VersionDetail,
  type VersionInfo,
} from "@/lib/api";
import { formatDate, formatDuration, formatLanguage, formatTime, formatUsd, speakerColor } from "@/lib/format";
import { defaultOptions, normalizeOptions } from "@/lib/options";
import { findGaps, gapKey, isMissing } from "@/lib/breaks";
import { useJobs } from "@/stores/jobs";
import { useLibrary } from "@/stores/library";
import { useSettings } from "@/stores/settings";

const route = useRoute();
const router = useRouter();
const jobs = useJobs();
const library = useLibrary();
const settingsStore = useSettings();

const id = computed(() => route.params.id as string);
const t = ref<TranscriptDetail | null>(null);
const title = ref("");
const folderId = ref<number | null>(null);
const tagIds = ref<number[]>([]);

const audio = ref<HTMLAudioElement | null>(null);
const currentTime = ref(0);
const audioDuration = ref(0);
const playing = ref(false);
const rate = ref("1");
const follow = ref(true);
const collapseBreaks = ref(true);

/** Archived versions, and the one being viewed (null = current). */
const versions = ref<VersionInfo[]>([]);
const viewing = ref<VersionDetail | null>(null);

/** Typed text; `find` is its debounced value (keeps typing smooth in long transcripts). */
const findInput = ref("");
const find = refDebounced(findInput, 150);
const onlyMatches = ref(false);
const editingId = ref<number | null>(null);
const speakerMenu = ref<{ seg: SegmentRow; x: number; y: number } | null>(null);
const speakerMenuEl = ref<HTMLElement | null>(null);
onClickOutside(speakerMenuEl, () => (speakerMenu.value = null));
useEventListener(window, "keydown", (e: KeyboardEvent) => e.key === "Escape" && (speakerMenu.value = null));

function openSpeakerMenu(seg: SegmentRow, anchor: HTMLElement) {
  const r = anchor.getBoundingClientRect();
  speakerMenu.value = { seg, x: r.left, y: Math.min(r.bottom + 4, window.innerHeight - 260) };
}
const renaming = ref<string | null>(null);
const identifying = ref(false);
const diarizing = ref(false);
/** Local speaker-finding job for this transcript (status stays "done" while it runs). */
const diarizeJob = computed(() => (job.value?.stage === "diarize" ? job.value : undefined));
const busy = computed(() => t.value?.status === "processing" || t.value?.status === "queued" || !!diarizeJob.value);

function openIdentify() {
  audio.value?.pause();
  identifying.value = true;
}
const ai = useAi();

/** English translation (AI), shown under each line when on. Made once per transcript version. */
const showEnglish = ref(false);
const translations = ref<Record<number, string> | null>(null);
const translating = computed(() => ai.running[`translate:${id.value}`]);
async function loadTranslation(): Promise<boolean> {
  const d = (await api.aiGetDigests(id.value)).find((x) => x.kind === "translation_en");
  if (!d || d.stale) return false;
  try {
    translations.value = JSON.parse(d.content);
    return true;
  } catch {
    return false;
  }
}
async function toggleEnglish() {
  if (showEnglish.value) {
    showEnglish.value = false;
    return;
  }
  showEnglish.value = true;
  if (await loadTranslation()) return;
  try {
    await api.aiTranslate(id.value);
    toast("Translating to English", { description: "Runs once per transcript; lines appear when it finishes." });
  } catch (e) {
    showEnglish.value = false;
    toast.error("Couldn't start the translation", { description: String(e) });
  }
}
watch(() => ai.digestVersion[id.value], () => { if (showEnglish.value) void loadTranslation(); });
watch(id, () => { showEnglish.value = false; translations.value = null; });

/** Copy the visible lines as "[time] Name: text" for pasting into another tool. */
async function copyAsContext() {
  if (!t.value) return;
  const lines = shown.value.map((s) => `[${formatTime(s.start)}] ${s.speaker ? `${speakerName(s.speaker)}: ` : ""}${s.text}`);
  const head = `# ${t.value.title}${t.value.recorded_at ? ` (${formatDate(t.value.recorded_at)})` : ""}\n\n`;
  await navigator.clipboard.writeText(head + lines.join("\n"));
  toast.success(`Copied ${lines.length} line(s)`);
}

const briefing = ref(false);
async function exportBrief() {
  if (!t.value || briefing.value) return;
  briefing.value = true;
  toast("Writing the project brief…", { description: "About half a minute." });
  try {
    const md = await api.aiBrief({ kind: "transcript", id: t.value.id });
    const path = await save({ defaultPath: `${t.value.title} — brief.md`, filters: [{ name: "Markdown", extensions: ["md"] }] });
    if (path) {
      await api.saveText(path, md);
      toast.success("Brief exported");
    }
  } catch (e) {
    toast.error("Brief failed", { description: String(e) });
  } finally {
    briefing.value = false;
  }
}

/** Right-hand assistant panel (summary + questions); ⌘J toggles it. */
const aiPanel = useLocalStorage("transcriber.aiPanel", false);
useEventListener(window, "keydown", (e: KeyboardEvent) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "j") {
    e.preventDefault();
    aiPanel.value = !aiPanel.value;
  }
});
function openCitation(otherId: string, time: number) {
  router.push({ name: "transcript", params: { id: otherId }, query: { t: String(time) } });
}

const retry = ref<JobOptions | null>(null);
const fixing = ref(false);
const checking = ref(false);

async function checkNow() {
  checking.value = true;
  try {
    await api.checkBatches();
    await load();
  } finally {
    checking.value = false;
  }
}

async function openFix() {
  await settingsStore.load();
  fixing.value = true;
}

/** Versions in the compare dialog: an archived version id, or "current". */
const comparing = ref<{ from: string; to: string } | null>(null);
const canCompare = computed(() => versions.value.length + (t.value?.segments.length ? 1 : 0) >= 2);

/** Compare an archived version (default: the newest) with the current one. */
function openCompare(versionId = versions.value[0]?.id) {
  if (versionId === undefined) return;
  const other = t.value?.segments.length ? "current" : String(versions.value.find((v) => v.id !== versionId)?.id);
  comparing.value = { from: String(versionId), to: other };
}

const job = computed(() => jobs.progress[id.value]);

async function load() {
  const d = await api.getTranscript(id.value);
  if (!d) {
    router.replace({ name: "library" });
    return;
  }
  t.value = d;
  versions.value = await api.listVersions(d.id);
  if (viewing.value && !versions.value.some((v) => v.id === viewing.value!.id)) viewing.value = null;
  title.value = d.title;
  folderId.value = d.folder_id;
  tagIds.value = d.tags.map((x) => x.id);
}

onMounted(async () => {
  await load();
  jumpFromQuery();
});
watch(id, async () => {
  await load();
  jumpFromQuery();
});
watch(() => route.query, jumpFromQuery);
// Show newly finished chunks while the job runs.
watch(
  () => job.value?.message,
  (m) => {
    if (m?.startsWith("Transcribed chunk")) void load();
  },
);
// Reload when this transcript's job finishes.
watch(job, (now, before) => {
  if (before && !now) void load();
});

/**
 * macOS can't open long AAC recordings with high sample rates (too many frames to index), so on
 * a load error the app switches to an MP3 playback copy, made once and kept next to the original.
 */
const playbackPath = ref<string | null>(null);
const preparingAudio = ref(false);
const audioError = ref<string | null>(null);
watch(id, () => {
  playbackPath.value = null;
  audioError.value = null;
});
const src = computed(() => (t.value ? convertFileSrc(playbackPath.value ?? t.value.audio_path) : ""));
async function onAudioError() {
  if (!t.value || playbackPath.value || preparingAudio.value) {
    if (playbackPath.value) audioError.value = "This recording can't be played.";
    return;
  }
  const tid = t.value.id;
  preparingAudio.value = true;
  try {
    const path = await api.playbackAudio(tid);
    if (tid === id.value) playbackPath.value = path;
  } catch (e) {
    audioError.value = `Couldn't prepare the audio for playback: ${e}`;
  } finally {
    preparingAudio.value = false;
  }
}
const duration = computed(() => t.value?.duration || audioDuration.value);

/** Loudness over time for the timeline's level lane and the meter (computed once per recording). */
const levels = ref<AudioLevels | null>(null);
watch(
  id,
  async (tid) => {
    levels.value = null;
    try {
      const lv = await api.audioLevels(tid);
      if (tid === id.value) levels.value = lv;
    } catch (e) {
      console.warn("audio levels failed", e);
    }
  },
  { immediate: true },
);

/** Segments/speakers of the version on screen. */
/** While a job runs or after it failed (with nothing saved yet), show the chunks finished so far. */
const showingPartial = computed(
  () => !viewing.value && !!t.value && !t.value.segments.length && !!t.value.partial?.segments.length,
);
const partialRows = computed<SegmentRow[]>(() =>
  (t.value?.partial?.segments ?? []).map((s, i) => ({ ...s, id: -(i + 1), idx: i })),
);
const partialUntil = computed(() => partialRows.value.at(-1)?.end ?? 0);
const segments = computed<SegmentRow[]>(
  () => viewing.value?.segments ?? (showingPartial.value ? partialRows.value : t.value?.segments ?? []),
);
const readOnly = computed(() => !!viewing.value || showingPartial.value);
/** Length used for the timeline and pause markers: not-yet-transcribed audio isn't a "break". */
const shownDuration = computed(() => (showingPartial.value ? partialUntil.value : duration.value));

/** Gaps (by `gapKey`) where the audio has speech but there's no text: skipped, not a pause. */
const missingGaps = ref(new Set<number>());
watch(
  [id, segments, shownDuration],
  async ([tid, segs, dur]) => {
    missingGaps.value = new Set();
    const gaps = findGaps(segs, dur);
    if (!tid || !gaps.length) return;
    try {
      const speech = await api.speechInGaps(tid, gaps.map((g) => [g.start, g.end]));
      if (tid !== id.value || segs !== segments.value || !Array.isArray(speech)) return; // stale
      missingGaps.value = new Set(gaps.filter((g, i) => isMissing(g, speech[i])).map(gapKey));
    } catch (e) {
      console.warn("speech check failed", e);
    }
  },
  { immediate: true },
);

const speakerOrder = computed(() => {
  const order: string[] = [];
  for (const s of segments.value) if (s.speaker && !order.includes(s.speaker)) order.push(s.speaker);
  return order;
});
const names = computed<Record<string, string>>(() =>
  Object.fromEntries((viewing.value?.speakers ?? t.value?.speakers ?? []).map((s) => [s.label, s.display_name])),
);
const speakerName = (label: string | null) => (label ? names.value[label] ?? label : "Unlabeled");

const matches = computed(() => {
  const q = find.value.trim().toLowerCase();
  if (!q) return new Set<number>();
  const words = q.split(/\s+/);
  return new Set(segments.value.filter((s) => words.every((w) => s.text.toLowerCase().includes(w))).map((s) => s.id));
});
const filtering = computed(() => onlyMatches.value && !!find.value.trim());
const shown = computed(() => (filtering.value ? segments.value.filter((s) => matches.value.has(s.id)) : segments.value));
const list = ref<InstanceType<typeof TranscriptList> | null>(null);

const activeId = computed(() => {
  const segs = segments.value;
  let found: SegmentRow | undefined;
  for (const s of segs) {
    if (s.start <= currentTime.value + 0.05) found = s;
    else break;
  }
  return found && currentTime.value <= found.end + 1.5 ? found.id : null;
});
watch(activeId, (sid) => {
  if (sid !== null && follow.value && playing.value) void list.value?.scrollToSegment(sid, true);
});

function jumpFromQuery() {
  const tq = Number(route.query.t);
  if (!Number.isNaN(tq) && route.query.t !== undefined) {
    seek(tq, false);
    // The list may only mount on the next render (right after loading).
    void nextTick(() => list.value?.scrollToSegment(Number(route.query.seg)));
  }
}

function seek(time: number, play = true) {
  const a = audio.value;
  if (!a) return;
  a.currentTime = time;
  currentTime.value = time;
  if (play) void a.play().catch(() => {});
}

function togglePlay() {
  const a = audio.value;
  if (!a) return;
  a.paused ? void a.play().catch(() => {}) : a.pause();
}
watch(rate, (r) => audio.value && (audio.value.playbackRate = Number(r)));

function onKey(e: KeyboardEvent) {
  const tag = (e.target as HTMLElement).tagName;
  if (tag === "INPUT" || tag === "TEXTAREA") return;
  if (e.code === "Space") {
    e.preventDefault();
    togglePlay();
  } else if (e.key === "ArrowLeft") seek(Math.max(0, currentTime.value - 5));
  else if (e.key === "ArrowRight") seek(currentTime.value + 5);
}

// --- metadata ---------------------------------------------------------------

async function saveMeta() {
  if (!t.value) return;
  const newTitle = title.value.trim() || t.value.title;
  title.value = newTitle;
  await api.updateTranscript(t.value.id, newTitle, folderId.value, tagIds.value);
  await library.refresh();
}
watch([folderId, tagIds], ([f, tags]) => {
  if (!t.value) return;
  const changed = f !== t.value.folder_id || tags.join() !== t.value.tags.map((x) => x.id).join();
  if (changed) void saveMeta().then(load);
});

async function renameSpeaker(name: string) {
  if (!t.value || !renaming.value) return;
  await api.renameSpeaker(t.value.id, renaming.value, name);
  renaming.value = null;
  await load();
}

async function saveSegment(seg: SegmentRow, text: string) {
  // Ignore a second save from the same edit (e.g. blur after Enter, or after Esc cancelled it).
  if (editingId.value !== seg.id) return;
  editingId.value = null;
  const next = text.trim();
  if (!next || seg.text === next) return;
  await api.updateSegment(seg.id, next, seg.speaker);
  seg.text = next;
}

async function reassign(seg: SegmentRow, label: string) {
  speakerMenu.value = null;
  await api.updateSegment(seg.id, seg.text, label);
  seg.speaker = label;
}

async function viewVersion(v: VersionInfo | null) {
  editingId.value = null;
  viewing.value = v ? await api.getVersion(v.id) : null;
}

async function restoreViewed() {
  if (!viewing.value) return;
  const n = viewing.value.number;
  await api.restoreVersion(viewing.value.id);
  viewing.value = null;
  await Promise.all([load(), library.refresh()]);
  toast.success(`Restored version ${n}`, { description: "The previous current version was kept in the history." });
}

async function deleteViewed() {
  if (!viewing.value) return;
  await api.deleteVersion(viewing.value.id);
  viewing.value = null;
  await load();
}

async function doExport(format: ExportFormat) {
  if (!t.value) return;
  const suffix = viewing.value ? ` (v${viewing.value.number})` : "";
  const path = await save({
    defaultPath: `${t.value.title}${suffix}.${format}`,
    filters: [{ name: format.toUpperCase(), extensions: [format] }],
  });
  if (!path) return;
  try {
    await api.exportTranscript(t.value.id, format, path, viewing.value?.id ?? null);
    toast.success(`Exported ${format.toUpperCase()}`);
  } catch (e) {
    toast.error("Export failed", { description: String(e) });
  }
}

/** True when the retry dialog was pre-filled from the failed job, so its finished chunks get reused. */
const retryResumes = ref(false);

async function openRetry() {
  const s = await settingsStore.load();
  const last = t.value?.last_options;
  const fromLast = !!last && (t.value?.status !== "done" || !!t.value?.error);
  retryResumes.value = fromLast;
  // After a failure, reuse the exact same options so cached chunks match; otherwise start from defaults.
  retry.value = fromLast
    ? { ...defaultOptions(s, last!.provider_id ?? t.value?.provider), ...last, title: null, folder_id: null, tag_ids: [] }
    : defaultOptions(s, t.value?.provider);
}

async function startRetry() {
  if (!t.value || !retry.value) return;
  try {
    await api.retranscribe(t.value.id, normalizeOptions(retry.value));
    retry.value = null;
    await Promise.all([load(), library.refresh()]);
  } catch (e) {
    toast.error("Couldn't start", { description: String(e) });
  }
}

async function remove() {
  if (!t.value) return;
  await library.trash(t.value.id);
  toast.success("Moved to trash");
  router.push({ name: "library" });
}

async function toggleFlag() {
  if (!t.value) return;
  const flagged = !t.value.flagged;
  t.value.flagged = flagged;
  await library.setFlagged(t.value.id, flagged);
}

async function restore() {
  if (!t.value) return;
  await library.restore(t.value.id);
  t.value.trashed_at = null;
  toast.success("Restored");
}

const confirmDelete = ref(false);
async function deleteForever() {
  if (!t.value) return;
  confirmDelete.value = false;
  await library.deleteForever(t.value.id);
  router.push({ name: "library" });
}

const saved = computed(() =>
  t.value?.duration && t.value.processed_duration
    ? Math.round((1 - t.value.processed_duration / t.value.duration) * 100)
    : null,
);
</script>

<template>
  <div v-if="t" class="flex h-full flex-col" tabindex="-1" @keydown="onKey">
    <!-- Header -->
    <header class="flex items-center gap-2 border-b px-4 py-3" data-tauri-drag-region>
      <Button variant="ghost" size="icon" @click="router.push({ name: 'library' })"><ArrowLeft /></Button>
      <input
        v-model="title"
        class="hover:bg-accent/50 focus:bg-accent/50 min-w-0 flex-1 rounded-md px-2 py-1 text-lg font-semibold outline-none"
        @blur="saveMeta"
        @keydown.enter="($event.target as HTMLInputElement).blur()"
      />
      <div class="w-44"><FolderSelect v-model="folderId" /></div>
      <div class="w-56"><TagPicker v-model="tagIds" /></div>
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="outline" :class="viewing && 'border-amber-400 text-amber-700 dark:text-amber-400'" title="Transcription history">
            <History /> v{{ viewing?.number ?? t.version }}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-[26rem]">
          <DropdownMenuLabel>Transcription history</DropdownMenuLabel>
          <DropdownMenuItem class="items-start" @click="viewVersion(null)">
            <Check class="mt-0.5" :class="viewing && 'invisible'" />
            <div class="grid min-w-0">
              <span class="font-medium">v{{ t.version }} · current</span>
              <span class="text-muted-foreground truncate text-xs">
                {{ t.model }} · {{ formatLanguage(t.language) }} · {{ formatDate(t.transcribed_at ?? t.created_at) }}
              </span>
            </div>
          </DropdownMenuItem>
          <DropdownMenuItem v-for="v in versions" :key="v.id" class="items-start" @click="viewVersion(v)">
            <Check class="mt-0.5" :class="viewing?.id !== v.id && 'invisible'" />
            <div class="grid min-w-0">
              <span>v{{ v.number }}</span>
              <span class="text-muted-foreground truncate text-xs">
                {{ v.model }} · {{ formatLanguage(v.language) }} · {{ formatDate(v.transcribed_at) }} · {{ v.segment_count }} segments
              </span>
            </div>
          </DropdownMenuItem>
          <p v-if="!versions.length" class="text-muted-foreground px-2 py-1.5 text-xs">
            Re-transcribing keeps earlier versions here.
          </p>
          <template v-else>
            <DropdownMenuSeparator />
            <DropdownMenuItem :disabled="!canCompare" @click="openCompare(viewing?.id)"><GitCompare /> Compare versions…</DropdownMenuItem>
          </template>
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="outline" :disabled="!segments.length || showingPartial"><Download /> Export</Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem @click="doExport('txt')">Plain text (.txt)</DropdownMenuItem>
          <DropdownMenuItem @click="doExport('md')">Markdown (.md)</DropdownMenuItem>
          <DropdownMenuItem @click="doExport('srt')">Subtitles (.srt)</DropdownMenuItem>
          <DropdownMenuItem @click="doExport('vtt')">WebVTT (.vtt)</DropdownMenuItem>
          <DropdownMenuItem @click="doExport('json')">JSON (.json)</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem :disabled="briefing" @click="exportBrief"><FileText /> Project brief (.md)…</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Button
        variant="outline"
        :class="aiPanel && 'border-primary text-primary'"
        title="Summary and questions about this meeting (⌘J)"
        @click="aiPanel = !aiPanel"
      ><Sparkles /> Assistant</Button>
      <DropdownMenu>
        <DropdownMenuTrigger as-child><Button variant="ghost" size="icon"><MoreHorizontal /></Button></DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem :disabled="!segments.length" @click="copyAsContext"><ClipboardCopy /> Copy as text for AI</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem :disabled="t.status === 'processing' || t.status === 'queued' || !t.segments.length || !!viewing" @click="openFix">
            <Wrench /> Fix problems…
          </DropdownMenuItem>
          <DropdownMenuItem :disabled="!speakerOrder.length || readOnly" @click="openIdentify"><Users /> Identify speakers…</DropdownMenuItem>
          <DropdownMenuItem :disabled="busy || !t.segments.length || !!viewing" @click="diarizing = true"><UserSearch /> Find speakers on this Mac…</DropdownMenuItem>
          <DropdownMenuItem :disabled="t.status === 'processing' || t.status === 'queued'" @click="openRetry"><RefreshCw /> Re-transcribe all…</DropdownMenuItem>
          <DropdownMenuItem v-if="t.status === 'processing' || t.status === 'queued'" @click="jobs.cancel(t.id)">Cancel job</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem @click="toggleFlag"><Flag /> {{ t.flagged ? "Remove flag" : "Flag" }}</DropdownMenuItem>
          <DropdownMenuSeparator />
          <template v-if="t.trashed_at">
            <DropdownMenuItem @click="restore"><RotateCcw /> Restore from trash</DropdownMenuItem>
            <DropdownMenuItem variant="destructive" @click="confirmDelete = true"><Trash2 /> Delete forever…</DropdownMenuItem>
          </template>
          <DropdownMenuItem v-else variant="destructive" @click="remove"><Trash2 /> Move to trash</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </header>

    <div class="flex min-h-0 flex-1 overflow-hidden">
    <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
    <div class="flex items-center gap-3 border-b px-6 py-2">
      <TranscriptMeta :t="t" />
      <span class="text-muted-foreground ml-auto truncate text-xs" :title="t.source_name">{{ t.source_name }}</span>
    </div>

    <div
      v-if="viewing"
      class="flex items-center gap-3 border-b border-amber-200 bg-amber-50 px-6 py-2 text-sm dark:border-amber-900 dark:bg-amber-950/40"
    >
      <History class="size-4 shrink-0 text-amber-600" />
      <span class="min-w-0 truncate">
        Viewing <b>v{{ viewing.number }}</b> · {{ viewing.model }} · {{ formatLanguage(viewing.language) }} ·
        {{ formatDate(viewing.transcribed_at) }} — read-only
      </span>
      <div class="ml-auto flex shrink-0 gap-1.5">
        <Button v-if="t.segments.length" size="sm" variant="outline" @click="openCompare(viewing.id)">
          <GitCompare /> Compare with current
        </Button>
        <Button size="sm" @click="restoreViewed"><RotateCcw /> Restore this version</Button>
        <Button size="sm" variant="ghost" @click="deleteViewed"><Trash2 /> Delete</Button>
        <Button size="sm" variant="outline" @click="viewVersion(null)">Back to current</Button>
      </div>
    </div>
    <div
      v-else-if="t.trashed_at"
      class="flex items-center gap-3 border-b border-amber-200 bg-amber-50 px-6 py-2 text-sm dark:border-amber-900 dark:bg-amber-950/40"
    >
      <Trash2 class="size-4 shrink-0 text-amber-600" />
      <span class="min-w-0 truncate">This transcript is in the trash (since {{ formatDate(t.trashed_at) }}).</span>
      <div class="ml-auto flex shrink-0 gap-1.5">
        <Button size="sm" @click="restore"><RotateCcw /> Restore</Button>
        <Button size="sm" variant="ghost" @click="confirmDelete = true"><Trash2 /> Delete forever</Button>
      </div>
    </div>
    <div
      v-else-if="t.notes && t.status === 'done' && !t.error"
      class="flex items-start gap-3 border-b border-amber-200 bg-amber-50 px-6 py-2 text-sm dark:border-amber-900 dark:bg-amber-950/40"
    >
      <AlertTriangle class="mt-0.5 size-4 shrink-0 text-amber-600" />
      <div class="min-w-0">
        <p>Some parts may be incomplete — the model still had problems after re-trying them in smaller pieces:</p>
        <ul class="text-muted-foreground mt-0.5 list-disc pl-5 text-xs">
          <li v-for="n in t.notes.split('\n')" :key="n">{{ n }}</li>
        </ul>
      </div>
      <Button size="sm" variant="outline" class="ml-auto shrink-0" @click="openFix"><Wrench /> Fix these parts</Button>
    </div>
    <div
      v-else-if="t.status === 'done' && t.error"
      class="flex items-center gap-3 border-b border-amber-200 bg-amber-50 px-6 py-2 text-sm dark:border-amber-900 dark:bg-amber-950/40"
    >
      <AlertTriangle class="size-4 shrink-0 text-amber-600" />
      <span class="min-w-0 truncate" :title="t.error">Last re-transcription failed — still showing v{{ t.version }}. {{ t.error }}</span>
      <Button size="sm" variant="outline" class="ml-auto shrink-0" @click="openRetry"><RefreshCw /> Retry</Button>
    </div>

    <div v-if="diarizeJob" class="flex items-center gap-3 border-b px-6 py-2 text-sm">
      <Loader2 class="text-primary size-4 shrink-0 animate-spin" />
      <span class="min-w-0 truncate">{{ jobs.cancelling.has(t.id) ? "Cancelling…" : diarizeJob.message }}</span>
      <Progress :model-value="diarizeJob.progress * 100" class="h-1.5 w-56" />
      <Button size="sm" variant="ghost" class="ml-auto shrink-0" :disabled="jobs.cancelling.has(t.id)" @click="jobs.cancel(t.id)">Cancel</Button>
    </div>

    <!-- Player + timeline -->
    <section class="border-b px-6 py-4">
      <audio
        ref="audio"
        :src="src"
        preload="metadata"
        @error="onAudioError"
        @timeupdate="currentTime = ($event.target as HTMLAudioElement).currentTime"
        @loadedmetadata="audioDuration = ($event.target as HTMLAudioElement).duration"
        @play="playing = true"
        @pause="playing = false"
      />
      <div class="mb-3 flex flex-wrap items-center gap-x-3 gap-y-1">
        <Button size="icon" class="rounded-full" @click="togglePlay">
          <Pause v-if="playing" /><Play v-else />
        </Button>
        <span class="text-sm tabular-nums">{{ formatTime(currentTime) }} / {{ formatTime(duration) }}</span>
        <LevelMeter :levels="levels" :audio="audio" :current-time="currentTime" :playing="playing" />
        <span v-if="preparingAudio" class="text-muted-foreground flex items-center gap-1 text-xs">
          <Loader2 class="size-3 animate-spin" /> Preparing audio for playback (one time, about a minute)…
        </span>
        <span v-else-if="audioError" class="text-destructive text-xs">{{ audioError }}</span>
        <Select v-model="rate">
          <SelectTrigger size="sm" class="w-20"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem v-for="r in ['0.75', '1', '1.25', '1.5', '2']" :key="r" :value="r">{{ r }}×</SelectItem>
          </SelectContent>
        </Select>
        <Button
          variant="ghost"
          size="sm"
          :class="follow ? 'text-primary' : 'text-muted-foreground'"
          title="Auto-scroll to the current segment"
          @click="follow = !follow"
        ><LocateFixed /> Follow</Button>
        <Button
          variant="ghost"
          size="sm"
          :class="collapseBreaks ? 'text-primary' : 'text-muted-foreground'"
          title="Shrink long stretches without speech on the timeline"
          @click="collapseBreaks = !collapseBreaks"
        ><Shrink /> Collapse breaks</Button>
        <Button
          v-if="segments.length"
          variant="ghost"
          size="sm"
          :class="showEnglish ? 'text-primary' : 'text-muted-foreground'"
          title="Show an English translation under each line (made once by the AI assistant)"
          @click="toggleEnglish"
        ><Loader2 v-if="showEnglish && translating" class="animate-spin" /><Languages v-else /> English</Button>
        <span class="text-muted-foreground ml-auto text-xs whitespace-nowrap">
          {{ t.provider }}
          <template v-if="saved !== null"> · {{ formatDuration(t.duration) }} → {{ formatDuration(t.processed_duration) }} sent ({{ saved }}% less)</template>
          <template v-if="t.usage.jobs">
            ·
            <span
              class="text-foreground font-medium"
              :title="`${t.usage.jobs} job(s), ${t.usage.requests} request(s)` + (t.usage.input_tokens ? `, ${t.usage.input_tokens} in / ${t.usage.output_tokens} out tokens` : '') + ` · saved ${formatUsd(t.usage.saved_usd)}`"
            >{{ formatUsd(t.usage.cost_usd) }}</span>
          </template>
        </span>
      </div>
      <SpeakerTimeline
        v-if="segments.length"
        :segments="segments"
        :collapse="collapseBreaks"
        :missing="missingGaps"
        :duration="shownDuration"
        :current-time="currentTime"
        :playing="playing"
        :levels="levels"
        :speaker-order="speakerOrder"
        :names="names"
        :matches="matches"
        @seek="seek"
      />
    </section>

    <!-- Status -->
    <div v-if="t.status === 'queued'" class="m-6 flex items-start gap-3 rounded-xl border p-4">
      <Moon class="text-primary mt-0.5 size-5 shrink-0" />
      <div class="min-w-0 flex-1">
        <p class="font-medium">Overnight batch — waiting at Google</p>
        <p class="text-muted-foreground mt-1 text-sm">
          {{ job?.message ?? "Google usually finishes within 24 hours, often much sooner." }}
        </p>
        <p v-if="t.batch" class="text-muted-foreground mt-1 text-xs">
          Sent {{ formatDate(t.batch.submitted_at) }} · {{ t.batch.chunks }} chunk(s) · {{ t.batch.model }} · the app checks every few minutes while it's open
        </p>
      </div>
      <div class="flex shrink-0 gap-2">
        <Button variant="outline" size="sm" :disabled="checking" @click="checkNow"><RefreshCw /> Check now</Button>
        <Button variant="ghost" size="sm" :disabled="jobs.cancelling.has(t.id)" @click="jobs.cancel(t.id)">
          {{ jobs.cancelling.has(t.id) ? "Cancelling…" : "Cancel" }}
        </Button>
      </div>
    </div>
    <div v-else-if="t.status === 'processing'" class="flex flex-col items-center justify-center gap-3" :class="showingPartial ? 'border-b p-4' : 'p-10'">
      <Loader2 class="text-primary size-8 animate-spin" />
      <p class="text-sm">{{ jobs.cancelling.has(t.id) ? "Cancelling…" : job?.message ?? "Processing…" }}</p>
      <Progress :model-value="(job?.progress ?? 0.02) * 100" class="h-1.5 w-80" />
      <Button variant="ghost" size="sm" :disabled="jobs.cancelling.has(t.id)" @click="jobs.cancel(t.id)">
        {{ jobs.cancelling.has(t.id) ? "Cancelling…" : "Cancel" }}
      </Button>
    </div>
    <div v-else-if="t.status === 'error'" class="m-6 flex items-start gap-3 rounded-xl border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950/40">
      <AlertCircle class="text-destructive mt-0.5 size-5 shrink-0" />
      <div class="min-w-0 flex-1">
        <p class="font-medium">Transcription failed</p>
        <p class="text-muted-foreground mt-1 text-sm break-words whitespace-pre-wrap">{{ t.error }}</p>
      </div>
      <Button variant="outline" @click="openRetry"><RefreshCw /> Retry</Button>
    </div>
    <div
      v-if="showingPartial && t.partial"
      class="mx-6 mb-2 flex items-center gap-2 rounded-lg border border-dashed px-4 py-2 text-sm"
      :class="t.status === 'error' ? '-mt-3' : 'mt-3'"
    >
      <Loader2 v-if="t.status === 'processing'" class="text-muted-foreground size-4 animate-spin" />
      <span>
        Partial transcript: <b>{{ t.partial.done }} of {{ t.partial.total }}</b> chunks done, up to
        <b class="tabular-nums">{{ formatTime(partialUntil) }}</b> of {{ formatTime(duration) }}.
        <template v-if="t.status === 'error'">
          <b>Retry</b> finishes the rest; finished chunks aren't sent (or billed) again.
        </template>
        <template v-else>More appears as chunks finish.</template>
      </span>
    </div>

    <!-- Transcript -->
    <template v-if="segments.length">
      <div class="flex items-center gap-2 px-6 pt-4">
        <div class="relative w-72">
          <Search class="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2" />
          <Input v-model="findInput" placeholder="Find in transcript" class="h-8 pl-8" />
        </div>
        <template v-if="find.trim()">
          <span class="text-muted-foreground text-xs">{{ matches.size }} match(es)</span>
          <Button variant="ghost" size="sm" @click="onlyMatches = !onlyMatches">
            {{ onlyMatches ? "Show all" : "Only matches" }}
          </Button>
        </template>
        <div class="ml-auto flex flex-wrap items-center gap-1.5">
          <button
            v-for="l in speakerOrder"
            :key="l"
            class="hover:bg-accent flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs disabled:pointer-events-none"
            :title="readOnly ? '' : 'Rename speaker'"
            :disabled="readOnly"
            @click="renaming = l"
          >
            <span class="size-2 rounded-full" :style="{ backgroundColor: speakerColor(l, speakerOrder) }" />
            {{ speakerName(l) }}
          </button>
          <Button
            v-if="speakerOrder.length && !readOnly"
            variant="outline"
            size="sm"
            title="Listen to a clip of each speaker and name them"
            @click="openIdentify"
          ><Users /> Identify speakers…</Button>
          <Button
            v-else-if="!readOnly && !busy"
            variant="outline"
            size="sm"
            title="Find who spoke when with the local diarizer (no API needed)"
            @click="diarizing = true"
          ><UserSearch /> Find speakers…</Button>
        </div>
      </div>

      <TranscriptList
        ref="list"
        :segments="shown"
        :breaks="!filtering"
        :missing="missingGaps"
        :duration="shownDuration"
        :active-id="activeId"
        :query="find"
        :matches="matches"
        :speaker-order="speakerOrder"
        :names="names"
        :read-only="readOnly"
        :editing-id="editingId"
        :translations="showEnglish ? translations : null"
        @seek="seek"
        @speaker-menu="openSpeakerMenu"
        @edit="(seg) => (editingId = seg.id)"
        @save="saveSegment"
        @cancel="editingId = null"
      />
    </template>

    </div>
    <aside v-if="aiPanel" class="bg-card w-[26rem] shrink-0 border-l">
      <AiPanel
        :t="t"
        :names="names"
        :read-only="readOnly"
        @seek="(time) => seek(time, false)"
        @open="openCitation"
        @refresh="load"
        @close="aiPanel = false"
      />
    </aside>
    </div>

    <div
      v-if="speakerMenu"
      ref="speakerMenuEl"
      class="bg-popover text-popover-foreground fixed z-50 min-w-44 rounded-lg border p-1 text-sm shadow-md"
      :style="{ left: `${speakerMenu.x}px`, top: `${speakerMenu.y}px` }"
    >
      <p class="text-muted-foreground px-2 py-1.5 text-xs font-medium">Assign speaker</p>
      <button
        v-for="l in speakerOrder"
        :key="l"
        class="hover:bg-accent flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left"
        @click="reassign(speakerMenu.seg, l)"
      >
        <span class="size-2 rounded-full" :style="{ backgroundColor: speakerColor(l, speakerOrder) }" />
        {{ speakerName(l) }}
        <Check v-if="speakerMenu.seg.speaker === l" class="ml-auto size-3.5" />
      </button>
      <template v-if="speakerMenu.seg.speaker">
        <div class="bg-border -mx-1 my-1 h-px" />
        <button
          class="hover:bg-accent w-full rounded-md px-2 py-1.5 text-left"
          @click="(renaming = speakerMenu.seg.speaker), (speakerMenu = null)"
        >Rename “{{ speakerName(speakerMenu.seg.speaker) }}”…</button>
      </template>
    </div>

    <ConfirmDialog
      v-if="confirmDelete"
      title="Delete forever?"
      description="The transcript and its audio copy will be removed. This can't be undone."
      confirm-label="Delete forever"
      destructive
      @close="confirmDelete = false"
      @confirm="deleteForever"
    />
    <FixProblemsDialog
      v-if="fixing && settingsStore.settings"
      :t="t"
      :settings="settingsStore.settings"
      @close="fixing = false"
      @seek="(time) => seek(time, false)"
      @started="(fixing = false), load(), library.refresh()"
    />

    <CompareDialog
      v-if="comparing"
      :t="t"
      :versions="versions"
      :from="comparing.from"
      :to="comparing.to"
      :playing="playing"
      @close="comparing = null"
      @seek="seek"
      @pause="audio?.pause()"
    />

    <SpeakerIdDialog
      v-if="identifying"
      :transcript-id="t.id"
      :segments="segments"
      :speaker-order="speakerOrder"
      :names="names"
      :src="src"
      @close="identifying = false"
      @saved="(identifying = false), load()"
    />

    <DiarizeDialog
      v-if="diarizing"
      :transcript-id="t.id"
      :duration="duration"
      :speaker-count="speakerOrder.length"
      @close="diarizing = false"
      @started="diarizing = false"
    />

    <PromptDialog
      v-if="renaming"
      title="Rename speaker"
      :initial="speakerName(renaming)"
      @close="renaming = null"
      @submit="renameSpeaker"
    />

    <Dialog :open="!!retry" @update:open="(o) => !o && (retry = null)">
      <DialogContent class="sm:max-w-lg">
        <DialogHeader><DialogTitle>Re-transcribe</DialogTitle></DialogHeader>
        <p v-if="retryResumes" class="text-muted-foreground -mt-2 text-xs">
          Same settings as the failed run, so chunks that already finished are reused (not re-sent or re-billed).
          Changing provider, model, language, speed or silence settings starts over.
        </p>
        <OptionsForm v-if="retry && settingsStore.settings" v-model="retry" :settings="settingsStore.settings" />
        <DialogFooter>
          <Button variant="ghost" @click="retry = null">Cancel</Button>
          <Button @click="startRetry">Start</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
