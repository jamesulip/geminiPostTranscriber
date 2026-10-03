<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { toast } from "vue-sonner";
import { Download, Loader2 } from "@lucide/vue";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api } from "@/lib/api";

/**
 * Start the local diarizer (sherpa-onnx: pyannote segmentation + speaker embeddings). It runs on
 * this Mac, so nothing is sent anywhere and it costs nothing; the models download once.
 */
const props = defineProps<{
  transcriptId: string;
  duration: number;
  /** Speakers in the current labeling, used as the default count. */
  speakerCount: number;
}>();
const emit = defineEmits<{ close: []; started: [] }>();

const count = ref(String(Math.max(2, props.speakerCount || 2)));
const modelsReady = ref<boolean | null>(null);
const starting = ref(false);
onMounted(async () => {
  modelsReady.value = await api.diarizationModelsReady().catch(() => false);
});

const n = computed(() => {
  const v = Number(count.value);
  return Number.isInteger(v) && v >= 1 && v <= 20 ? v : null;
});
/** About 12× realtime on an M-series Mac. */
const minutes = computed(() => Math.max(1, Math.round(props.duration / 12 / 60)));

async function start() {
  if (n.value === null) return;
  starting.value = true;
  try {
    await api.diarize(props.transcriptId, n.value);
    emit("started");
  } catch (e) {
    toast.error("Couldn't start", { description: String(e) });
  } finally {
    starting.value = false;
  }
}
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>Find speakers on this Mac</DialogTitle>
        <DialogDescription>
          Listens to the whole recording to tell voices apart, then relabels each line with the voice that spoke it.
          Runs locally, free. The current labels are kept in the history.
        </DialogDescription>
      </DialogHeader>
      <form class="grid gap-4" @submit.prevent="start">
        <label class="grid gap-1.5 text-sm">
          <span class="font-medium">Number of speakers</span>
          <Input v-model="count" type="number" min="1" max="20" class="w-24" autofocus />
          <span class="text-muted-foreground text-xs">
            Count everyone who talks, including brief ones. Too few merges voices; too many splits one person in two.
            Tiny leftover "speakers" can be merged afterwards from the transcript.
          </span>
        </label>
        <ul class="text-muted-foreground grid gap-1 text-xs">
          <li class="flex items-center gap-1.5">
            <Loader2 v-if="modelsReady === null" class="size-3 animate-spin" />
            <Download v-else-if="!modelsReady" class="size-3" />
            <template v-if="modelsReady === false">First run downloads the speaker models once (about 45 MB).</template>
            <template v-else-if="modelsReady">Speaker models are ready.</template>
          </li>
          <li>Takes about {{ minutes }} min for this recording; you can keep using the app meanwhile.</li>
        </ul>
        <DialogFooter>
          <Button type="button" variant="ghost" @click="emit('close')">Cancel</Button>
          <Button type="submit" :disabled="n === null || starting"><Loader2 v-if="starting" class="animate-spin" /> Find speakers</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
