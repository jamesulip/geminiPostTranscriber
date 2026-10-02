<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { toast } from "vue-sonner";
import { Loader2, Plus, Trash2 } from "@lucide/vue";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import OptionsForm from "./OptionsForm.vue";
import { api, type JobOptions, type ProblemRange, type Settings, type TranscriptDetail } from "@/lib/api";
import { formatTime, formatUsd, parseClock } from "@/lib/format";
import { defaultOptions, normalizeOptions, providerDefaults } from "@/lib/options";

const props = defineProps<{ t: TranscriptDetail; settings: Settings }>();
const emit = defineEmits<{ close: []; started: []; seek: [time: number] }>();

type Row = ProblemRange & { selected: boolean; manual?: boolean };
const rows = ref<Row[]>([]);
const loading = ref(true);
const busy = ref(false);
const manualStart = ref("");
const manualEnd = ref("");
const pricePerMinute = ref(0);

// Same options as the last full run (provider, speed, silence…), so the fix matches the rest.
const opts = ref<JobOptions>({
  ...defaultOptions(props.settings, props.t.last_options?.provider_id ?? props.t.provider),
  ...(props.t.last_options ?? {}),
  title: null,
  folder_id: null,
  tag_ids: [],
});

// Use today's (smaller, more reliable) chunk size even if the last run used longer chunks.
{
  const p = props.settings.providers.find((x) => x.id === opts.value.provider_id);
  opts.value.chunk_minutes = Math.min(opts.value.chunk_minutes, providerDefaults(props.settings, p).chunk_minutes);
}

onMounted(async () => {
  try {
    const found = await api.findProblems(props.t.id);
    rows.value = found.map((r) => ({ ...r, selected: r.kind === "text" }));
  } catch (e) {
    toast.error("Couldn't check the transcript", { description: String(e) });
  } finally {
    loading.value = false;
  }
  const p = props.settings.providers.find((x) => x.id === opts.value.provider_id);
  if (p) pricePerMinute.value = p.price_per_minute ?? (await api.knownPrice(opts.value.model || p.model)).per_minute;
});

const selected = computed(() => rows.value.filter((r) => r.selected));
const selectedSecs = computed(() => selected.value.reduce((a, r) => a + (r.end - r.start), 0));
/** Rough share of audio actually sent after silence removal + speed-up, from the last run. */
const sendRatio = computed(() =>
  props.t.duration && props.t.processed_duration ? props.t.processed_duration / props.t.duration : 0.6,
);
const estimate = computed(() => (selectedSecs.value * sendRatio.value * pricePerMinute.value) / 60);
const fullEstimate = computed(() => (props.t.duration * sendRatio.value * pricePerMinute.value) / 60);

const manualError = computed(() => {
  if (!manualStart.value && !manualEnd.value) return "";
  const a = parseClock(manualStart.value);
  const b = parseClock(manualEnd.value);
  if (a === null || b === null) return "Use times like 1:32:00 or 32:00";
  if (b <= a) return "End must be after start";
  if (b > props.t.duration + 1) return `The recording ends at ${formatTime(props.t.duration)}`;
  return "";
});

function addManual() {
  const a = parseClock(manualStart.value);
  const b = parseClock(manualEnd.value);
  if (a === null || b === null || manualError.value) return;
  rows.value.push({ start: a, end: Math.min(b, props.t.duration), reasons: ["added by you"], kind: "text", selected: true, manual: true });
  rows.value.sort((x, y) => x.start - y.start);
  manualStart.value = manualEnd.value = "";
}

async function start() {
  // Overlapping selections are merged so no stretch is sent twice.
  const ranges: [number, number][] = [];
  for (const r of [...selected.value].sort((x, y) => x.start - y.start)) {
    const last = ranges.at(-1);
    if (last && r.start <= last[1]) last[1] = Math.max(last[1], r.end);
    else ranges.push([r.start, r.end]);
  }
  busy.value = true;
  try {
    await api.repairTranscript(props.t.id, ranges, normalizeOptions(opts.value));
    emit("started");
  } catch (e) {
    toast.error("Couldn't start", { description: String(e) });
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
      <DialogHeader>
        <DialogTitle>Fix problems</DialogTitle>
        <DialogDescription>
          Re-transcribe only the parts that went wrong. Everything else is kept, and the current version stays in the history.
        </DialogDescription>
      </DialogHeader>

      <div v-if="loading" class="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 class="size-4 animate-spin" /> Checking the transcript against the audio…
      </div>
      <template v-else>
        <p v-if="!rows.length" class="text-muted-foreground rounded-lg border border-dashed p-4 text-sm">
          No problems found. You can still add a part below.
        </p>
        <div v-else class="divide-y rounded-lg border">
          <label v-for="(r, i) in rows" :key="`${r.start}-${i}`" class="flex cursor-pointer items-start gap-3 p-3">
            <Checkbox v-model="r.selected" class="mt-0.5" />
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2 text-sm">
                <button class="hover:text-primary font-medium tabular-nums" title="Jump there" @click.prevent="emit('seek', r.start)">
                  {{ formatTime(r.start) }} – {{ formatTime(r.end) }}
                </button>
                <span class="text-muted-foreground text-xs">{{ Math.round((r.end - r.start) / 60) || "<1" }} min</span>
                <span
                  class="rounded px-1.5 text-[10px] font-medium"
                  :class="r.kind === 'speakers' ? 'bg-muted text-muted-foreground' : 'bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300'"
                >{{ r.kind === "speakers" ? "speaker labels" : "text" }}</span>
              </div>
              <p class="text-muted-foreground text-xs">{{ r.reasons.join(" · ") }}</p>
            </div>
            <Button v-if="r.manual" variant="ghost" size="icon" class="size-7" @click.prevent="rows.splice(i, 1)"><Trash2 /></Button>
          </label>
        </div>

        <div class="grid gap-1.5">
          <p class="text-sm font-medium">Add a part</p>
          <div class="flex items-center gap-2">
            <Input v-model="manualStart" placeholder="from, e.g. 1:30:00" class="h-8 w-40" @keydown.enter="addManual" />
            <span class="text-muted-foreground text-sm">to</span>
            <Input v-model="manualEnd" placeholder="to, e.g. 1:40:00" class="h-8 w-40" @keydown.enter="addManual" />
            <Button variant="outline" size="sm" :disabled="!manualStart || !manualEnd || !!manualError" @click="addManual"><Plus /> Add</Button>
          </div>
          <p v-if="manualError" class="text-destructive text-xs">{{ manualError }}</p>
        </div>

        <details class="rounded-lg border p-3">
          <summary class="cursor-pointer text-sm font-medium">Transcription settings</summary>
          <p class="text-muted-foreground mt-1 mb-3 text-xs">Pre-filled from the last full run so the fixed parts match the rest.</p>
          <OptionsForm v-model="opts" :settings="settings" />
        </details>
      </template>

      <DialogFooter class="items-center">
        <p v-if="selected.length" class="text-muted-foreground mr-auto text-xs">
          {{ selected.length }} part(s), {{ Math.round(selectedSecs / 60) }} min of recording
          <template v-if="pricePerMinute"> · ≈ {{ formatUsd(estimate) }} (whole recording ≈ {{ formatUsd(fullEstimate) }})</template>
        </p>
        <Button variant="ghost" @click="emit('close')">Cancel</Button>
        <Button :disabled="loading || busy || !selected.length" @click="start">Fix {{ selected.length || "" }} part(s)</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
