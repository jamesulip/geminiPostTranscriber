<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { convertFileSrc } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { toast } from "vue-sonner";
import {
  AlertCircle, ArrowLeft, Download, Loader2, LocateFixed, MoreHorizontal, Pause, Play, RefreshCw, Search, Trash2,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Textarea } from "@/components/ui/textarea";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import FolderSelect from "@/components/FolderSelect.vue";
import TagPicker from "@/components/TagPicker.vue";
import SpeakerTimeline from "@/components/SpeakerTimeline.vue";
import OptionsForm from "@/components/OptionsForm.vue";
import PromptDialog from "@/components/PromptDialog.vue";
import { api, type ExportFormat, type JobOptions, type SegmentRow, type TranscriptDetail } from "@/lib/api";
import { formatDuration, formatTime, formatUsd, speakerColor } from "@/lib/format";
import { highlightHtml } from "@/lib/highlight";
import { defaultOptions, normalizeOptions } from "@/lib/options";
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

const find = ref("");
const onlyMatches = ref(false);
const editing = ref<{ id: number; text: string } | null>(null);
const renaming = ref<string | null>(null);
const retry = ref<JobOptions | null>(null);

const job = computed(() => jobs.progress[id.value]);

async function load() {
  const d = await api.getTranscript(id.value);
  if (!d) {
    router.replace({ name: "library" });
    return;
  }
  t.value = d;
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
// Reload when this transcript's job finishes.
watch(job, (now, before) => {
  if (before && !now) void load();
});

const src = computed(() => (t.value ? convertFileSrc(t.value.audio_path) : ""));
const duration = computed(() => t.value?.duration || audioDuration.value);

const speakerOrder = computed(() => {
  const order: string[] = [];
  for (const s of t.value?.segments ?? []) if (s.speaker && !order.includes(s.speaker)) order.push(s.speaker);
  return order;
});
const names = computed<Record<string, string>>(() =>
  Object.fromEntries((t.value?.speakers ?? []).map((s) => [s.label, s.display_name])),
);
const speakerName = (label: string | null) => (label ? names.value[label] ?? label : "Unlabeled");

const matches = computed(() => {
  const q = find.value.trim().toLowerCase();
  if (!q || !t.value) return new Set<number>();
  const words = q.split(/\s+/);
  return new Set(t.value.segments.filter((s) => words.every((w) => s.text.toLowerCase().includes(w))).map((s) => s.id));
});
const shown = computed(() => {
  const segs = t.value?.segments ?? [];
  return onlyMatches.value && find.value.trim() ? segs.filter((s) => matches.value.has(s.id)) : segs;
});

const activeId = computed(() => {
  const segs = t.value?.segments ?? [];
  let found: SegmentRow | undefined;
  for (const s of segs) {
    if (s.start <= currentTime.value + 0.05) found = s;
    else break;
  }
  return found && currentTime.value <= found.end + 1.5 ? found.id : null;
});
watch(activeId, (sid) => {
  if (sid !== null && follow.value && playing.value) {
    document.getElementById(`seg-${sid}`)?.scrollIntoView({ block: "center", behavior: "smooth" });
  }
});

function jumpFromQuery() {
  const tq = Number(route.query.t);
  if (!Number.isNaN(tq) && route.query.t !== undefined) {
    seek(tq, false);
    nextTick(() =>
      document.getElementById(`seg-${route.query.seg}`)?.scrollIntoView({ block: "center" }),
    );
  }
}

function seek(time: number, play = true) {
  const a = audio.value;
  if (!a) return;
  a.currentTime = time;
  currentTime.value = time;
  if (play) void a.play();
}

function togglePlay() {
  const a = audio.value;
  if (!a) return;
  a.paused ? void a.play() : a.pause();
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

async function saveSegment() {
  const e = editing.value;
  if (!e || !t.value) return;
  const seg = t.value.segments.find((s) => s.id === e.id);
  editing.value = null;
  if (!seg || seg.text === e.text.trim()) return;
  await api.updateSegment(seg.id, e.text.trim(), seg.speaker);
  seg.text = e.text.trim();
}

async function reassign(seg: SegmentRow, label: string) {
  await api.updateSegment(seg.id, seg.text, label);
  seg.speaker = label;
}

async function doExport(format: ExportFormat) {
  if (!t.value) return;
  const path = await save({
    defaultPath: `${t.value.title}.${format}`,
    filters: [{ name: format.toUpperCase(), extensions: [format] }],
  });
  if (!path) return;
  try {
    await api.exportTranscript(t.value.id, format, path);
    toast.success(`Exported ${format.toUpperCase()}`);
  } catch (e) {
    toast.error("Export failed", { description: String(e) });
  }
}

async function openRetry() {
  const s = await settingsStore.load();
  const o = defaultOptions(s, t.value?.provider);
  retry.value = o;
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
  await api.deleteTranscript(t.value.id);
  await library.refresh();
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
          <Button variant="outline" :disabled="t.status !== 'done'"><Download /> Export</Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem @click="doExport('txt')">Plain text (.txt)</DropdownMenuItem>
          <DropdownMenuItem @click="doExport('md')">Markdown (.md)</DropdownMenuItem>
          <DropdownMenuItem @click="doExport('srt')">Subtitles (.srt)</DropdownMenuItem>
          <DropdownMenuItem @click="doExport('vtt')">WebVTT (.vtt)</DropdownMenuItem>
          <DropdownMenuItem @click="doExport('json')">JSON (.json)</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu>
        <DropdownMenuTrigger as-child><Button variant="ghost" size="icon"><MoreHorizontal /></Button></DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem :disabled="t.status === 'processing'" @click="openRetry"><RefreshCw /> Re-transcribe…</DropdownMenuItem>
          <DropdownMenuItem v-if="t.status === 'processing'" @click="api.cancelJob(t.id)">Cancel job</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" @click="remove"><Trash2 /> Delete</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </header>

    <!-- Player + timeline -->
    <section class="border-b px-6 py-4">
      <audio
        ref="audio"
        :src="src"
        preload="metadata"
        @timeupdate="currentTime = ($event.target as HTMLAudioElement).currentTime"
        @loadedmetadata="audioDuration = ($event.target as HTMLAudioElement).duration"
        @play="playing = true"
        @pause="playing = false"
      />
      <div class="mb-3 flex items-center gap-3">
        <Button size="icon" class="rounded-full" @click="togglePlay">
          <Pause v-if="playing" /><Play v-else />
        </Button>
        <span class="text-sm tabular-nums">{{ formatTime(currentTime) }} / {{ formatTime(duration) }}</span>
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
        <span class="text-muted-foreground ml-auto text-xs">
          {{ t.provider }} · {{ t.model }}
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
        v-if="t.segments.length"
        :segments="t.segments"
        :duration="duration"
        :current-time="currentTime"
        :speaker-order="speakerOrder"
        :names="names"
        :matches="matches"
        @seek="seek"
      />
    </section>

    <!-- Status -->
    <div v-if="t.status === 'processing'" class="flex flex-col items-center justify-center gap-3 p-10">
      <Loader2 class="text-primary size-8 animate-spin" />
      <p class="text-sm">{{ job?.message ?? "Processing…" }}</p>
      <Progress :model-value="(job?.progress ?? 0.02) * 100" class="h-1.5 w-80" />
      <Button variant="ghost" size="sm" @click="api.cancelJob(t.id)">Cancel</Button>
    </div>
    <div v-else-if="t.status === 'error'" class="m-6 flex items-start gap-3 rounded-xl border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950/40">
      <AlertCircle class="text-destructive mt-0.5 size-5 shrink-0" />
      <div class="min-w-0 flex-1">
        <p class="font-medium">Transcription failed</p>
        <p class="text-muted-foreground mt-1 text-sm break-words whitespace-pre-wrap">{{ t.error }}</p>
      </div>
      <Button variant="outline" @click="openRetry"><RefreshCw /> Retry</Button>
    </div>

    <!-- Transcript -->
    <template v-if="t.segments.length">
      <div class="flex items-center gap-2 px-6 pt-4">
        <div class="relative w-72">
          <Search class="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2" />
          <Input v-model="find" placeholder="Find in transcript" class="h-8 pl-8" />
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
            class="hover:bg-accent flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs"
            title="Rename speaker"
            @click="renaming = l"
          >
            <span class="size-2 rounded-full" :style="{ backgroundColor: speakerColor(l, speakerOrder) }" />
            {{ speakerName(l) }}
          </button>
        </div>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto px-6 py-3">
        <div
          v-for="s in shown"
          :id="`seg-${s.id}`"
          :key="s.id"
          class="group grid grid-cols-[64px_120px_1fr] gap-3 rounded-lg px-2 py-1.5"
          :class="s.id === activeId ? 'bg-primary/10' : 'hover:bg-accent/40'"
        >
          <button class="text-muted-foreground hover:text-primary pt-0.5 text-left text-xs tabular-nums" @click="seek(s.start)">
            {{ formatTime(s.start) }}
          </button>
          <DropdownMenu>
            <DropdownMenuTrigger as-child>
              <button class="flex h-fit items-center gap-1.5 truncate pt-0.5 text-left text-xs font-medium">
                <span class="size-2 shrink-0 rounded-full" :style="{ backgroundColor: speakerColor(s.speaker, speakerOrder) }" />
                <span class="truncate">{{ speakerName(s.speaker) }}</span>
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              <DropdownMenuLabel>Assign speaker</DropdownMenuLabel>
              <DropdownMenuItem v-for="l in speakerOrder" :key="l" @click="reassign(s, l)">{{ speakerName(l) }}</DropdownMenuItem>
              <DropdownMenuSeparator v-if="s.speaker" />
              <DropdownMenuItem v-if="s.speaker" @click="renaming = s.speaker">Rename “{{ speakerName(s.speaker) }}”…</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Textarea
            v-if="editing?.id === s.id"
            v-model="editing.text"
            autofocus
            class="min-h-0 text-sm"
            @blur="saveSegment"
            @keydown.enter.exact.prevent="saveSegment"
            @keydown.esc="editing = null"
          />
          <p
            v-else
            class="cursor-text text-sm leading-relaxed"
            title="Double-click to edit"
            @dblclick="editing = { id: s.id, text: s.text }"
            v-html="highlightHtml(s.text, find)"
          />
        </div>
      </div>
    </template>

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
        <OptionsForm v-if="retry && settingsStore.settings" v-model="retry" :settings="settingsStore.settings" />
        <DialogFooter>
          <Button variant="ghost" @click="retry = null">Cancel</Button>
          <Button @click="startRetry">Start</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
