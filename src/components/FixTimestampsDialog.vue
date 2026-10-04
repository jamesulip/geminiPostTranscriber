<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { toast } from "vue-sonner";
import { Clock, Loader2 } from "@lucide/vue";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { api, type Settings, type TranscriptDetail } from "@/lib/api";
import { formatUsd } from "@/lib/format";
import { defaultOptions, normalizeOptions } from "@/lib/options";

/**
 * Re-time every line against word timings from a Whisper provider. General models (Gemini
 * Flash) write timestamps themselves and drift by up to a minute, so clicking a line plays
 * someone else. The text and speakers stay; the current timing is kept as a version.
 */
const props = defineProps<{ t: TranscriptDetail; settings: Settings }>();
const emit = defineEmits<{ close: []; started: [] }>();

/** Whisper-style providers with a key (gpt-4o-* return text without timings). */
const providers = computed(() =>
  props.settings.providers.filter((p) => p.kind === "openai" && p.api_key.trim() && !p.model.startsWith("gpt-4o")),
);
const providerId = ref(
  providers.value.find((p) => p.id === props.settings.default_provider)?.id ?? providers.value[0]?.id ?? "",
);
const provider = computed(() => providers.value.find((p) => p.id === providerId.value));

const pricePerMinute = ref(0);
watch(
  provider,
  async (p) => {
    pricePerMinute.value = p ? (p.price_per_minute ?? (await api.knownPrice(p.model)).per_minute) : 0;
  },
  { immediate: true },
);
/** Rough share of audio sent after silence removal + speed-up, from the last run. */
const sendRatio = computed(() => (props.t.duration && props.t.processed_duration ? props.t.processed_duration / props.t.duration : 0.7));
const estimate = computed(() => (props.t.duration * sendRatio.value * pricePerMinute.value) / 60);

const busy = ref(false);
async function start() {
  if (!provider.value) return;
  busy.value = true;
  try {
    const opts = { ...defaultOptions(props.settings, provider.value.id), language: props.t.language ?? props.settings.language ?? null };
    await api.fixTimestamps(props.t.id, normalizeOptions(opts));
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
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>Fix timestamps</DialogTitle>
        <DialogDescription>
          Moves each line to where its words are actually spoken, using word timings from a Whisper model. Use it when clicking
          a line (or a speaker clip) plays something else.
        </DialogDescription>
      </DialogHeader>
      <div class="grid gap-3 text-sm">
        <p class="text-muted-foreground text-xs">
          The text and speakers stay as they are. The current timing is kept in the transcription history. The summary will
          show as outdated (it cites times) and is redone when next opened; an English translation stays as it is.
        </p>
        <div v-if="providers.length" class="grid gap-1.5">
          <span class="text-xs font-medium">Time the words with</span>
          <Select v-model="providerId">
            <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem v-for="p in providers" :key="p.id" :value="p.id">{{ p.name }} · {{ p.model }}</SelectItem>
            </SelectContent>
          </Select>
          <span v-if="pricePerMinute" class="text-muted-foreground text-xs">≈ {{ formatUsd(estimate) }} for this recording</span>
        </div>
        <p v-else class="rounded-lg border border-dashed p-3 text-xs">
          Needs a Whisper provider with an API key (for example Groq). Add one in Settings.
        </p>
      </div>
      <DialogFooter>
        <Button variant="ghost" @click="emit('close')">Cancel</Button>
        <Button :disabled="!provider || busy" @click="start">
          <Loader2 v-if="busy" class="animate-spin" /><Clock v-else /> Fix timestamps
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
