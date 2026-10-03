<script setup lang="ts">
import { computed, markRaw, nextTick, ref, shallowRef, toRaw, watch } from "vue";
import { useVirtualizer } from "@tanstack/vue-virtual";
import { ArrowLeftRight, ChevronDown, ChevronUp, Loader2, Pause, Play, UnfoldVertical } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { api, type Speaker, type TranscriptDetail, type VersionInfo } from "@/lib/api";
import { DEFAULT_WINDOW, diffRows, diffTranscripts, type DiffResult, type DiffSide } from "@/lib/diff";
import { formatDate, formatLanguage, formatTime } from "@/lib/format";

// "git diff" between two transcriptions of this recording (any archived version or the current one).
const props = defineProps<{
  t: TranscriptDetail;
  versions: VersionInfo[];
  /** Versions compared at first: an archived version id, or "current". */
  from: string;
  to: string;
  playing: boolean;
}>();
const emit = defineEmits<{ close: []; seek: [time: number]; pause: [] }>();

const CURRENT = "current";
const names = (speakers: Speaker[]) => Object.fromEntries(speakers.map((s) => [s.label, s.display_name]));

// Snapshot of the current version (and its label), so a job finishing while the dialog is open
// doesn't change what's compared. Kept raw: thousands of segments don't need to be reactive.
const current: DiffSide | null = props.t.segments.length
  ? markRaw({ segments: toRaw(props.t.segments).map((s) => ({ ...toRaw(s) })), names: names(props.t.speakers) })
  : null;
const currentLabel = {
  label: `v${props.t.version} · current`,
  detail: `${props.t.model} · ${formatLanguage(props.t.language)} · ${formatDate(props.t.transcribed_at ?? props.t.created_at)}`,
};
const cache = new Map<string, DiffSide>();

async function side(key: string): Promise<DiffSide> {
  if (key === CURRENT && current) return current;
  let s = cache.get(key);
  if (!s) {
    const v = await api.getVersion(Number(key));
    s = markRaw({ segments: v?.segments ?? [], names: names(v?.speakers ?? []) });
    cache.set(key, s);
  }
  return s;
}

const choices = computed(() => [
  ...(current ? [{ key: CURRENT, ...currentLabel }] : []),
  ...props.versions.map((v) => ({
    key: String(v.id),
    label: `v${v.number}`,
    detail: `${v.model} · ${formatLanguage(v.language)} · ${formatDate(v.transcribed_at)}`,
  })),
]);

const from = ref(props.from);
const to = ref(props.to);
const loose = ref(true);
const speakers = ref(false);

const result = shallowRef<DiffResult | null>(null);
const hasSpeakers = ref(false);
const loading = ref(false);
const failed = ref<string | null>(null);
/** Collapsed unchanged stretches the user opened (by first block index). */
const expanded = shallowRef(new Set<number>());
const scroller = ref<HTMLElement | null>(null);

const view = computed(() => (result.value ? diffRows(result.value.blocks, expanded.value) : { rows: [], hunks: [] }));

function expand(from: number) {
  expanded.value = new Set(expanded.value).add(from);
}

const virtualizer = useVirtualizer(
  computed(() => ({
    count: view.value.rows.length,
    getScrollElement: () => scroller.value,
    getItemKey: (i: number) => view.value.rows[i].key,
    estimateSize: () => 28,
    overscan: 12,
    // Layout size, unaffected by the dialog's zoom-in animation.
    measureElement: (el: Element) => (el as HTMLElement).offsetHeight,
  })),
);
const items = computed(() => virtualizer.value.getVirtualItems().map((item) => ({ item, row: view.value.rows[item.index] })));

let run = 0;
watch(
  [from, to, loose, speakers],
  async () => {
    const n = ++run;
    failed.value = null;
    if (from.value === to.value) {
      result.value = null;
      return;
    }
    loading.value = true;
    try {
      const [a, b] = await Promise.all([side(from.value), side(to.value)]);
      if (n !== run) return;
      result.value = markRaw(diffTranscripts(a, b, { loose: loose.value, speakers: speakers.value, window: DEFAULT_WINDOW }));
      hasSpeakers.value = [a, b].some((x) => x.segments.some((s) => s.speaker));
      expanded.value = new Set();
      await nextTick();
      // Row keys repeat between diffs, so drop row heights measured for the previous one.
      virtualizer.value.measure();
      virtualizer.value.scrollToOffset(0);
    } catch (e) {
      if (n === run) failed.value = String(e);
    } finally {
      if (n === run) loading.value = false;
    }
  },
  { immediate: true },
);

/** Index of the first row at the top of the list. */
function topRow() {
  const v = virtualizer.value;
  const offset = v.scrollOffset ?? 0;
  return v.getVirtualItems().find((i) => i.end > offset + 1)?.index ?? 0;
}
/** Changes at or above the top of the list ("3 of 120"). */
const position = computed(() => {
  void items.value; // update while scrolling
  const top = topRow();
  return view.value.hunks.filter((h) => h <= top).length;
});

function go(step: 1 | -1) {
  const hunks = view.value.hunks;
  const top = topRow();
  const target = step > 0 ? hunks.find((h) => h > top) : hunks.filter((h) => h < top).at(-1);
  if (target !== undefined) virtualizer.value.scrollToIndex(target, { align: "start" });
}

/** Never round a small difference up to "100%". */
const percent = (x: number) => (x === 1 ? "100%" : x > 0.99 ? `${Math.min(99.9, Math.floor(x * 1000) / 10)}%` : `${Math.round(x * 100)}%`);

const lineClass = { "-": "bg-red-500/10", "+": "bg-green-500/10", " ": "text-muted-foreground" } as const;
const markClass = { "-": "rounded-sm bg-red-500/30", "+": "rounded-sm bg-green-500/30", " ": "" } as const;
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="flex h-[85vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-5xl">
      <DialogHeader class="gap-1 border-b p-4 pr-12">
        <DialogTitle>Compare versions</DialogTitle>
        <DialogDescription>
          Like a git diff: red words are only in the first version, green words only in the second. Click a time to listen.
        </DialogDescription>
        <div class="mt-2 flex flex-wrap items-center gap-2">
          <Select v-model="from">
            <SelectTrigger size="sm" class="w-72"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem v-for="c in choices" :key="c.key" :value="c.key">
                {{ c.label }} <span class="text-muted-foreground text-xs">{{ c.detail }}</span>
              </SelectItem>
            </SelectContent>
          </Select>
          <Button variant="ghost" size="icon-sm" title="Swap" @click="[from, to] = [to, from]"><ArrowLeftRight /></Button>
          <Select v-model="to">
            <SelectTrigger size="sm" class="w-72"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem v-for="c in choices" :key="c.key" :value="c.key">
                {{ c.label }} <span class="text-muted-foreground text-xs">{{ c.detail }}</span>
              </SelectItem>
            </SelectContent>
          </Select>
          <div class="ml-auto flex items-center gap-4 text-xs">
            <label class="flex items-center gap-2"><Switch v-model="loose" /> Ignore case &amp; punctuation</label>
            <label class="flex items-center gap-2" title="Also mark words whose speaker differs"><Switch v-model="speakers" /> Speakers</label>
          </div>
        </div>
      </DialogHeader>

      <div class="flex min-h-10 items-center gap-3 border-b px-4 py-1.5 text-xs">
        <template v-if="from === to">Pick two different versions.</template>
        <template v-else-if="failed"><span class="text-destructive">Couldn't load the version: {{ failed }}</span></template>
        <template v-else-if="loading"><Loader2 class="size-3.5 animate-spin" /> Comparing…</template>
        <template v-else-if="result">
          <span class="font-medium text-red-600 tabular-nums dark:text-red-400">−{{ result.removed.toLocaleString() }} words</span>
          <span class="font-medium text-green-600 tabular-nums dark:text-green-400">+{{ result.added.toLocaleString() }} words</span>
          <span class="text-muted-foreground">
            {{ percent(result.similarity) }} the same · {{ view.hunks.length.toLocaleString() }} change{{ view.hunks.length === 1 ? "" : "s" }}
          </span>
          <div class="ml-auto flex items-center gap-1.5">
            <Button v-if="playing" size="sm" variant="ghost" @click="emit('pause')"><Pause /> Pause</Button>
            <span v-if="view.hunks.length" class="text-muted-foreground tabular-nums">{{ position }} / {{ view.hunks.length.toLocaleString() }}</span>
            <Button size="icon-sm" variant="outline" title="Previous change" :disabled="!view.hunks.length" @click="go(-1)"><ChevronUp /></Button>
            <Button size="icon-sm" variant="outline" title="Next change" :disabled="!view.hunks.length" @click="go(1)"><ChevronDown /></Button>
          </div>
        </template>
      </div>
      <p v-if="result && !loading && !view.hunks.length" class="text-muted-foreground border-b px-4 py-2 text-xs">
        No differences in the words{{ loose ? " (ignoring case and punctuation)" : "" }}.
        <template v-if="!speakers && result.speakerChanges">
          {{ result.speakerChanges.toLocaleString() }} word(s) have a different speaker — turn on <b>Speakers</b> to see where.
        </template>
      </p>

      <!-- No browser scroll anchoring: it fights the virtualizer's own positioning on short jumps. -->
      <div ref="scroller" class="min-h-0 flex-1 overflow-y-auto [overflow-anchor:none]">
        <div class="relative w-full" :style="{ height: `${virtualizer.getTotalSize()}px` }">
          <div class="absolute inset-x-0 top-0" :style="{ transform: `translateY(${items[0]?.item.start ?? 0}px)` }">
            <div
              v-for="{ item, row } in items"
              :key="item.key as PropertyKey"
              :ref="(el) => virtualizer.measureElement(el as Element | null)"
              :data-index="item.index"
            >
              <button
                v-if="row.kind === 'hunk'"
                class="bg-muted/60 text-muted-foreground hover:text-foreground flex w-full items-center gap-2 border-y px-4 py-1 font-mono text-xs"
                title="Play from here"
                @click="emit('seek', row.start)"
              >
                <Play class="size-3" /> @@ {{ formatTime(row.start) }} – {{ formatTime(row.end) }} @@
                <span class="ml-auto tabular-nums">
                  <span class="text-red-600 dark:text-red-400">−{{ row.removed }}</span>
                  <span class="text-green-600 dark:text-green-400"> +{{ row.added }}</span>
                </span>
              </button>
              <div
                v-else-if="row.kind === 'line'"
                class="grid gap-2 px-4 py-0.5"
                :class="[lineClass[row.sign], hasSpeakers ? 'grid-cols-[12px_56px_100px_1fr]' : 'grid-cols-[12px_56px_1fr]']"
              >
                <span class="pt-0.5 font-mono text-xs select-none">{{ row.sign }}</span>
                <button class="text-muted-foreground hover:text-primary pt-0.5 text-left text-xs tabular-nums" @click="emit('seek', row.line.seg.start)">
                  {{ formatTime(row.line.seg.start) }}
                </button>
                <span
                  v-if="hasSpeakers"
                  class="h-fit truncate pt-0.5 text-xs"
                  :class="row.line.speakerChanged && 'rounded-sm bg-amber-500/30 px-1'"
                >{{ row.line.speaker }}</span>
                <p class="text-sm leading-relaxed"><template v-for="(p, k) in row.line.parts" :key="k">{{ k ? " " : "" }}<span v-if="p.changed" :class="markClass[row.sign]">{{ p.text }}</span><template v-else>{{ p.text }}</template></template></p>
              </div>
              <button
                v-else
                class="text-muted-foreground hover:bg-accent/50 hover:text-foreground flex w-full items-center justify-center gap-2 py-1.5 text-xs"
                title="Show these segments"
                @click="expand(row.from)"
              >
                <UnfoldVertical class="size-3.5" />
                {{ row.segments.toLocaleString() }} unchanged segment{{ row.segments === 1 ? "" : "s" }} · {{ formatTime(row.start) }} – {{ formatTime(row.end) }}
              </button>
            </div>
          </div>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
