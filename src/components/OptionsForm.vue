<script setup lang="ts">
import { computed, watch } from "vue";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { JobOptions, Settings } from "@/lib/api";

const opts = defineModel<JobOptions>({ required: true });
const props = defineProps<{ settings: Settings }>();

const provider = computed(() => props.settings.providers.find((p) => p.id === opts.value.provider_id));

watch(
  () => opts.value.provider_id,
  (id, old) => {
    if (!old || id === old) return;
    const p = props.settings.providers.find((x) => x.id === id);
    opts.value.model = p?.model ?? null;
    opts.value.chunk_minutes = p?.kind === "gemini" ? props.settings.gemini_chunk_minutes : props.settings.chunk_minutes;
  },
);

const speed = computed({
  get: () => [opts.value.speed],
  set: (v: number[] | undefined) => (opts.value.speed = v?.[0] ?? 1),
});
const threshold = computed({
  get: () => [opts.value.silence_threshold_db],
  set: (v: number[] | undefined) => (opts.value.silence_threshold_db = v?.[0] ?? -35),
});
const missingKey = computed(() => provider.value && !provider.value.api_key.trim());
</script>

<template>
  <div class="grid gap-4">
    <div class="grid grid-cols-2 gap-3">
      <div class="grid gap-1.5">
        <Label>Provider</Label>
        <Select v-model="opts.provider_id">
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
    <p v-else-if="provider?.kind === 'openai' && !opts.model?.includes('diarize')" class="text-muted-foreground -mt-2 text-xs">
      Whisper models don't label speakers. Use Gemini (or <code>gpt-4o-transcribe-diarize</code>) for diarization.
    </p>

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
        <span class="text-sm tabular-nums" :class="opts.speed > 1.5 && 'text-amber-600'">{{ opts.speed.toFixed(2) }}×</span>
      </div>
      <Slider v-model="speed" :min="1" :max="2" :step="0.05" />
      <p class="text-muted-foreground text-xs">
        Faster audio is cheaper to transcribe. Above 1.5× accuracy may drop.
      </p>
    </div>

    <div class="grid gap-2">
      <div class="flex items-center justify-between">
        <Label for="rm-silence">Remove silence</Label>
        <Switch id="rm-silence" v-model="opts.remove_silence" />
      </div>
      <template v-if="opts.remove_silence">
        <div class="text-muted-foreground flex items-center justify-between text-xs">
          <span>Silence threshold</span><span class="tabular-nums">{{ opts.silence_threshold_db }} dB</span>
        </div>
        <Slider v-model="threshold" :min="-60" :max="-20" :step="1" />
        <div class="text-muted-foreground flex items-center justify-between gap-3 text-xs">
          <span>Minimum silence length (s)</span>
          <Input v-model.number="opts.silence_min_duration" type="number" min="0.3" max="5" step="0.1" class="h-7 w-20" />
        </div>
      </template>
    </div>
  </div>
</template>
