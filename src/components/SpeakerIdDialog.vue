<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref } from "vue";
import { Loader2, Pause, Play, SkipForward } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, type SegmentRow } from "@/lib/api";
import { formatTime, speakerColor } from "@/lib/format";

/**
 * Put names to the speaker labels a diarizing model produced ("Speaker 1", …): one row per
 * speaker with a few representative clips to listen to and a name field. Enter saves the row and
 * jumps to the next speaker, playing one of their clips, so a meeting can be labeled in a minute.
 */
const props = defineProps<{
  transcriptId: string;
  segments: SegmentRow[];
  speakerOrder: string[];
  names: Record<string, string>;
  /** Audio URL of the main player (so the same playback copy is used). */
  src: string;
}>();
const emit = defineEmits<{ close: []; saved: [] }>();

const SAMPLES = 5;
/** Clips longer than this are cut off: a few seconds is enough to recognize a voice. */
const MAX_CLIP = 12;

/** A few clear clips per speaker, spread over the recording: long turns with real sentences. */
function pickSamples(segs: SegmentRow[]): SegmentRow[] {
  const good = segs.filter((s) => s.end - s.start >= 2.5 && s.text.trim().split(/\s+/).length >= 5);
  const pool = (good.length >= SAMPLES ? good : segs)
    .slice()
    .sort((a, b) => b.end - b.start - (a.end - a.start))
    .slice(0, SAMPLES * 3)
    .sort((a, b) => a.start - b.start);
  if (pool.length <= SAMPLES) return pool;
  const out = new Set<SegmentRow>();
  for (let i = 0; i < SAMPLES; i++) out.add(pool[Math.round((i * (pool.length - 1)) / (SAMPLES - 1))]);
  return [...out];
}

const total = computed(() => props.segments.reduce((a, s) => a + (s.end - s.start), 0) || 1);
const rows = computed(() =>
  props.speakerOrder.map((label) => {
    const segs = props.segments.filter((s) => s.speaker === label);
    const talk = segs.reduce((a, s) => a + (s.end - s.start), 0);
    return {
      label,
      color: speakerColor(label, props.speakerOrder),
      count: segs.length,
      talk,
      share: Math.round((talk / total.value) * 100),
      first: segs[0]?.start ?? 0,
      samples: pickSamples(segs),
    };
  }),
);

const drafts = reactive<Record<string, string>>(
  Object.fromEntries(props.speakerOrder.map((l) => [l, props.names[l] && props.names[l] !== l ? props.names[l] : ""])),
);
const sampleIdx = reactive<Record<string, number>>(Object.fromEntries(props.speakerOrder.map((l) => [l, 0])));
const changed = computed(() => props.speakerOrder.filter((l) => drafts[l].trim() && drafts[l].trim() !== (props.names[l] ?? l)));

// --- playback ------------------------------------------------------------------

const audio = ref<HTMLAudioElement | null>(null);
const current = ref<{ label: string; seg: SegmentRow; until: number } | null>(null);
const playing = ref(false);

function play(label: string, seg: SegmentRow) {
  const a = audio.value;
  if (!a) return;
  current.value = { label, seg, until: Math.min(seg.end, seg.start + MAX_CLIP) };
  a.currentTime = seg.start;
  void a.play().catch(() => {});
}
function toggle(label: string) {
  const row = rows.value.find((r) => r.label === label);
  const seg = row?.samples[sampleIdx[label]];
  if (!seg) return;
  if (current.value?.label === label && current.value.seg.id === seg.id && playing.value) audio.value?.pause();
  else play(label, seg);
}
function next(label: string) {
  const n = rows.value.find((r) => r.label === label)?.samples.length ?? 0;
  if (!n) return;
  sampleIdx[label] = (sampleIdx[label] + 1) % n;
  const seg = rows.value.find((r) => r.label === label)!.samples[sampleIdx[label]];
  play(label, seg);
}
function onTime() {
  const a = audio.value;
  const c = current.value;
  if (a && c && a.currentTime >= c.until) a.pause();
}
const clipProgress = computed(() => {
  const c = current.value;
  if (!c || !playing.value) return 0;
  return Math.min(1, (progressTime.value - c.seg.start) / (c.until - c.seg.start));
});
const progressTime = ref(0);
onBeforeUnmount(() => audio.value?.pause());

// --- naming ----------------------------------------------------------------------

const inputs = ref<Record<string, InstanceType<typeof Input>>>({});
const setInput = (label: string) => (el: unknown) => {
  if (el) inputs.value[label] = el as InstanceType<typeof Input>;
};

/** Enter: go to the next speaker and play one of their clips. On the last row, save. */
function onEnter(i: number) {
  const nextLabel = props.speakerOrder[i + 1];
  if (!nextLabel) {
    void save();
    return;
  }
  const el = (inputs.value[nextLabel]?.$el ?? null) as HTMLInputElement | null;
  el?.focus();
  el?.select();
  const seg = rows.value[i + 1].samples[sampleIdx[nextLabel]];
  if (seg) play(nextLabel, seg);
}

function onKey(e: KeyboardEvent) {
  const tag = (e.target as HTMLElement).tagName;
  if (e.code === "Space" && tag !== "INPUT" && tag !== "BUTTON" && current.value) {
    e.preventDefault();
    toggle(current.value.label);
  }
}

const saving = ref(false);
async function save() {
  if (saving.value) return;
  saving.value = true;
  try {
    for (const l of changed.value) await api.renameSpeaker(props.transcriptId, l, drafts[l].trim());
    if (changed.value.length) toast.success(`Named ${changed.value.length} speaker(s)`);
    emit("saved");
  } catch (e) {
    toast.error("Couldn't save names", { description: String(e) });
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="flex max-h-[85vh] flex-col sm:max-w-2xl" @keydown="onKey">
      <DialogHeader>
        <DialogTitle>Identify speakers</DialogTitle>
        <DialogDescription>
          Listen to a clip of each speaker and type their name. Enter moves to the next speaker and plays a clip of them;
          <kbd class="bg-muted rounded px-1">Space</kbd> pauses.
        </DialogDescription>
      </DialogHeader>
      <audio
        ref="audio"
        :src="src"
        preload="metadata"
        @timeupdate="(onTime(), (progressTime = ($event.target as HTMLAudioElement).currentTime))"
        @play="playing = true"
        @pause="playing = false"
        @ended="playing = false"
      />
      <div class="-mx-1 min-h-0 flex-1 overflow-y-auto px-1">
        <div
          v-for="(r, i) in rows"
          :key="r.label"
          class="grid grid-cols-[auto_1fr_minmax(0,16rem)] items-start gap-x-3 gap-y-1 border-b py-3 last:border-0"
          :class="current?.label === r.label && playing && 'bg-accent/40 -mx-2 rounded-lg px-2'"
        >
          <!-- Play controls -->
          <div class="flex items-center gap-1 pt-0.5">
            <Button
              size="icon-sm"
              class="rounded-full"
              :disabled="!r.samples.length"
              :title="current?.label === r.label && playing ? 'Pause' : 'Play a clip of this speaker'"
              @click="toggle(r.label)"
            >
              <Pause v-if="current?.label === r.label && playing" /><Play v-else />
            </Button>
            <Button variant="ghost" size="icon-sm" :disabled="r.samples.length < 2" title="Play another clip" @click="next(r.label)">
              <SkipForward />
            </Button>
          </div>
          <!-- Who -->
          <div class="min-w-0">
            <div class="flex items-center gap-2 text-sm">
              <span class="size-2.5 shrink-0 rounded-full" :style="{ backgroundColor: r.color }" />
              <span class="shrink-0 font-medium whitespace-nowrap">{{ names[r.label] && names[r.label] !== r.label ? names[r.label] : r.label }}</span>
              <span class="text-muted-foreground truncate text-xs tabular-nums">
                {{ r.count }} turns · {{ formatTime(r.talk) }} ({{ r.share }}%) · first at {{ formatTime(r.first) }}
              </span>
            </div>
            <template v-if="r.samples.length">
              <p class="text-muted-foreground mt-1 line-clamp-2 text-xs leading-snug">
                <span class="text-foreground/70 tabular-nums">{{ formatTime(r.samples[sampleIdx[r.label]].start) }}</span>
                “{{ r.samples[sampleIdx[r.label]].text }}”
              </p>
              <div class="mt-1 flex items-center gap-2">
                <div class="bg-muted h-1 w-32 overflow-hidden rounded-full">
                  <div
                    class="h-full rounded-full transition-[width] duration-200"
                    :style="{ width: `${current?.label === r.label ? clipProgress * 100 : 0}%`, backgroundColor: r.color }"
                  />
                </div>
                <span class="text-muted-foreground text-[11px] tabular-nums">clip {{ sampleIdx[r.label] + 1 }}/{{ r.samples.length }}</span>
              </div>
            </template>
          </div>
          <!-- Name -->
          <Input
            :ref="setInput(r.label)"
            v-model="drafts[r.label]"
            :placeholder="`Name for ${r.label}`"
            autocomplete="off"
            :autofocus="i === 0"
            class="h-8"
            :style="drafts[r.label].trim() && drafts[r.label].trim() !== (names[r.label] ?? r.label) ? { borderColor: r.color } : undefined"
            @keydown.enter.prevent="onEnter(i)"
          />
        </div>
      </div>
      <DialogFooter class="items-center">
        <span v-if="changed.length" class="text-muted-foreground mr-auto text-xs">{{ changed.length }} name(s) to save</span>
        <Button variant="ghost" @click="emit('close')">Cancel</Button>
        <Button :disabled="!changed.length || saving" @click="save">
          <Loader2 v-if="saving" class="animate-spin" /> Save names
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
