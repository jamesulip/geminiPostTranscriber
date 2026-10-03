<script setup lang="ts">
import { computed, watch } from "vue";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { diarizes, type JobOptions, type SilenceMethod, type Settings } from "@/lib/api";
import { batchOptions, defaultOptions, providerDefaults } from "@/lib/options";

const opts = defineModel<JobOptions>({ required: true });
const props = defineProps<{ settings: Settings }>();

const provider = computed(() => props.settings.providers.find((p) => p.id === opts.value.provider_id));
/** Provider with the model override applied (e.g. a typed-in `gpt-4o-transcribe-diarize`). */
const effective = computed(() =>
  provider.value ? { ...provider.value, model: opts.value.model?.trim() || provider.value.model } : undefined,
);

watch(
  () => opts.value.provider_id,
  (id, old) => {
    // Overnight mode sets its own provider/model/chunking (see setWhen).
    if (!old || id === old || opts.value.batch) return;
    const p = props.settings.providers.find((x) => x.id === id);
    opts.value.model = p?.model ?? null;
    Object.assign(opts.value, providerDefaults(props.settings, p));
  },
);

/** Picking speaker labels switches between the cheap default and the diarizing provider. */
const speakerLabels = computed({
  get: () => diarizes(effective.value),
  set: (on: boolean) => {
    const id = on ? props.settings.diarize_provider : props.settings.default_provider;
    if (props.settings.providers.some((p) => p.id === id)) opts.value.provider_id = id;
  },
});
const canToggleLabels = computed(
  () =>
    props.settings.providers.some((p) => p.id === props.settings.diarize_provider) &&
    props.settings.providers.some((p) => p.id === props.settings.default_provider) &&
    props.settings.diarize_provider !== props.settings.default_provider,
);

const speed = computed({
  get: () => [opts.value.speed],
  set: (v: number[] | undefined) => (opts.value.speed = v?.[0] ?? 1),
});
const threshold = computed({
  get: () => [opts.value.silence_threshold_db],
  set: (v: number[] | undefined) => (opts.value.silence_threshold_db = v?.[0] ?? -35),
});
const vad = computed({
  get: () => [opts.value.vad_threshold],
  set: (v: number[] | undefined) => (opts.value.vad_threshold = v?.[0] ?? 0.5),
});
const method = computed({
  get: () => opts.value.silence_method,
  set: (v: string) => (opts.value.silence_method = v as SilenceMethod),
});
/** "Now" (fast) vs "Overnight" (Gemini Batch API: ~50% cheaper, results within 24 h). */
const hasGemini = computed(() => props.settings.providers.some((p) => p.kind === "gemini"));
function setWhen(batch: boolean) {
  if (batch === opts.value.batch) return;
  const keep = { language: opts.value.language, title: opts.value.title, folder_id: opts.value.folder_id, tag_ids: opts.value.tag_ids };
  const base = defaultOptions(props.settings);
  opts.value = { ...(batch ? batchOptions(props.settings, base) : base), ...keep };
}

const providerName = (id: string) => props.settings.providers.find((p) => p.id === id)?.name ?? id;
const missingKey = computed(() => provider.value && !provider.value.api_key.trim());
</script>

<template>
  <div class="grid gap-4">
    <div class="grid gap-1.5">
      <Label>When</Label>
      <div class="grid grid-cols-2 gap-2">
        <button
          type="button"
          class="rounded-lg border p-3 text-left transition-colors"
          :class="!opts.batch ? 'border-primary bg-primary/5' : 'hover:bg-accent/50'"
          @click="setWhen(false)"
        >
          <p class="text-sm font-medium">Now</p>
          <p class="text-muted-foreground text-xs">Results in minutes. Subject to rate limits.</p>
        </button>
        <button
          type="button"
          class="rounded-lg border p-3 text-left transition-colors disabled:opacity-50"
          :class="opts.batch ? 'border-primary bg-primary/5' : 'hover:bg-accent/50'"
          :disabled="!hasGemini"
          @click="setWhen(true)"
        >
          <p class="text-sm font-medium">Overnight (batch)</p>
          <p class="text-muted-foreground text-xs">~50% cheaper, with speaker labels. Results within 24 h, often sooner.</p>
        </button>
      </div>
      <p v-if="opts.batch" class="text-muted-foreground text-xs">
        Uses <code>{{ opts.model }}</code> through Google's Batch API. You can close the app; it checks for results when
        it's open. Speaker labels and timings come from the model's prompt, so they're a little less precise than Gemini 3.5 Transcribe.
      </p>
    </div>

    <div v-if="!opts.batch" class="flex items-start justify-between gap-4 rounded-lg border p-3">
      <div class="grid gap-0.5">
        <Label for="speaker-labels">Speaker labels</Label>
        <p class="text-muted-foreground text-xs">
          {{ speakerLabels
            ? `Identifies who said what (${providerName(settings.diarize_provider)}). Roughly 8× the cost of text only.`
            : `Text only with ${providerName(settings.default_provider)} — the cheapest option.` }}
        </p>
      </div>
      <Switch id="speaker-labels" v-model="speakerLabels" :disabled="!canToggleLabels" />
    </div>

    <div class="grid grid-cols-2 gap-3">
      <div class="grid gap-1.5">
        <Label>Provider</Label>
        <Select v-model="opts.provider_id" :disabled="opts.batch">
          <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem v-for="p in settings.providers" :key="p.id" :value="p.id">{{ p.name }}</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div class="grid gap-1.5">
        <Label>Model</Label>
        <Input v-model="opts.model as string" :placeholder="provider?.model" />
      </div>
    </div>
    <p v-if="missingKey" class="text-destructive -mt-2 text-xs">No API key for {{ provider?.name }} — add one in Settings.</p>

    <div class="grid grid-cols-2 gap-3">
      <div class="grid gap-1.5">
        <Label>Language <span class="text-muted-foreground font-normal">(optional)</span></Label>
        <Input v-model="opts.language as string" placeholder="Auto-detect, e.g. en, fil" />
      </div>
      <div class="grid gap-1.5">
        <Label>Chunk length (min)</Label>
        <Input v-model.number="opts.chunk_minutes" type="number" min="1" max="30" step="1" />
      </div>
    </div>

    <div class="grid gap-2">
      <div class="flex items-center justify-between">
        <Label>Speed</Label>
        <span class="text-sm tabular-nums" :class="opts.speed > (speakerLabels ? 1.5 : 1.9) && 'text-amber-600'">
          {{ opts.speed.toFixed(2) }}×
        </span>
      </div>
      <Slider v-model="speed" :min="1" :max="2" :step="0.05" />
      <p class="text-muted-foreground text-xs">
        Faster audio is cheaper.
        {{ speakerLabels
          ? "Kept lower with speaker labels: voice identification hasn't been tested at high speed."
          : "Whisper accuracy holds up well to about 2×." }}
      </p>
    </div>

    <div class="grid gap-2">
      <div class="flex items-center justify-between">
        <Label for="rm-silence">Remove silence</Label>
        <Switch id="rm-silence" v-model="opts.remove_silence" />
      </div>
      <template v-if="opts.remove_silence">
        <div class="flex items-center justify-between gap-3 text-xs">
          <span class="text-muted-foreground">Detect</span>
          <Select v-model="method">
            <SelectTrigger size="sm" class="w-56"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="speech">Non-speech (AI)</SelectItem>
              <SelectItem value="volume">Quiet only (volume)</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <template v-if="opts.silence_method === 'speech'">
          <div class="text-muted-foreground flex items-center justify-between text-xs">
            <span>Speech threshold <span class="opacity-70">(lower keeps more)</span></span>
            <span class="tabular-nums">{{ opts.vad_threshold.toFixed(2) }}</span>
          </div>
          <Slider v-model="vad" :min="0.2" :max="0.8" :step="0.05" />
          <p class="text-muted-foreground text-xs">
            Also drops background noise, music and hum — cheaper, and fewer made-up lines from Whisper.
          </p>
        </template>
        <template v-else>
          <div class="text-muted-foreground flex items-center justify-between text-xs">
            <span>Silence threshold</span><span class="tabular-nums">{{ opts.silence_threshold_db }} dB</span>
          </div>
          <Slider v-model="threshold" :min="-60" :max="-20" :step="1" />
        </template>
        <div class="text-muted-foreground flex items-center justify-between gap-3 text-xs">
          <span>Minimum pause to remove (s)</span>
          <Input v-model.number="opts.silence_min_duration" type="number" min="0.3" max="5" step="0.1" class="h-7 w-20" />
        </div>
      </template>
    </div>
  </div>
</template>
