<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Loader2, Merge } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { api, type SegmentRow } from "@/lib/api";
import { formatTime, speakerColor } from "@/lib/format";

/**
 * Merge speaker labels that are really one person (diarizers often split a voice in two): tick
 * the speakers, name the result, and every turn of theirs gets one label. The first ticked
 * speaker's label is kept; the labels from before are archived as a version.
 */
const props = defineProps<{
  transcriptId: string;
  segments: SegmentRow[];
  speakerOrder: string[];
  names: Record<string, string>;
  /** Speaker to start with ticked (e.g. from a segment's speaker menu). */
  initial?: string | null;
}>();
const emit = defineEmits<{ close: []; merged: [versionId: number, summary: string] }>();

const nameOf = (l: string) => props.names[l] ?? l;
/** A name someone typed, not the diarizer's placeholder label. */
const isNamed = (l: string) => !!props.names[l] && props.names[l] !== l;

const rows = computed(() =>
  props.speakerOrder.map((label) => {
    const segs = props.segments.filter((s) => s.speaker === label);
    const longest = segs.reduce<SegmentRow | null>((a, s) => (!a || s.end - s.start > a.end - a.start ? s : a), null);
    return {
      label,
      color: speakerColor(label, props.speakerOrder),
      count: segs.length,
      talk: segs.reduce((a, s) => a + (s.end - s.start), 0),
      first: segs[0]?.start ?? 0,
      quote: longest?.text ?? "",
    };
  }),
);

const picked = ref<string[]>(props.initial ? [props.initial] : []);
const toggle = (label: string, on: boolean) => {
  picked.value = on ? props.speakerOrder.filter((l) => l === label || picked.value.includes(l)) : picked.value.filter((l) => l !== label);
};
/** The label that survives: the earliest speaker ticked. */
const into = computed(() => picked.value[0] ?? null);
const from = computed(() => picked.value.slice(1));

/** Suggested name: the first ticked speaker someone has named (so "Speaker 1" + "Alice" stays Alice). */
const suggested = computed(() => {
  const l = picked.value.find(isNamed) ?? into.value;
  return l ? nameOf(l) : "";
});
const name = ref(suggested.value);
const nameEdited = ref(false);
watch(suggested, (s) => {
  if (!nameEdited.value) name.value = s;
});

const turns = computed(() => rows.value.filter((r) => picked.value.includes(r.label)).reduce((a, r) => a + r.count, 0));
const canMerge = computed(() => picked.value.length >= 2 && !!name.value.trim());

const saving = ref(false);
async function merge() {
  if (!canMerge.value || !into.value || saving.value) return;
  saving.value = true;
  const final = name.value.trim();
  const others = [...new Set(picked.value.map(nameOf).filter((n) => n !== final))];
  try {
    const versionId = await api.mergeSpeakers(props.transcriptId, from.value, into.value, final);
    emit("merged", versionId, others.length ? `Merged ${others.join(", ")} into “${final}”` : `Merged ${picked.value.length} speakers named “${final}”`);
  } catch (e) {
    toast.error("Couldn't merge speakers", { description: String(e) });
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="flex max-h-[85vh] flex-col sm:max-w-xl">
      <DialogHeader>
        <DialogTitle>Merge speakers</DialogTitle>
        <DialogDescription>
          Tick the speakers who are the same person. All their turns get one label and one name.
        </DialogDescription>
      </DialogHeader>
      <form class="flex min-h-0 flex-1 flex-col gap-4" @submit.prevent="merge">
        <div class="-mx-1 min-h-0 flex-1 overflow-y-auto px-1">
          <label
            v-for="r in rows"
            :key="r.label"
            class="hover:bg-accent/40 flex cursor-pointer items-start gap-3 rounded-lg px-2 py-2"
            :class="picked.includes(r.label) && 'bg-accent/40'"
          >
            <Checkbox
              :model-value="picked.includes(r.label)"
              class="mt-0.5"
              @update:model-value="(v) => toggle(r.label, v === true)"
            />
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2 text-sm">
                <span class="size-2.5 shrink-0 rounded-full" :style="{ backgroundColor: r.color }" />
                <span class="shrink-0 font-medium whitespace-nowrap">{{ nameOf(r.label) }}</span>
                <span v-if="isNamed(r.label)" class="text-muted-foreground shrink-0 text-xs">{{ r.label }}</span>
                <span class="text-muted-foreground ml-auto shrink-0 text-xs tabular-nums">
                  {{ r.count }} {{ r.count === 1 ? "turn" : "turns" }} · {{ formatTime(r.talk) }} · first at {{ formatTime(r.first) }}
                </span>
              </div>
              <p v-if="r.quote" class="text-muted-foreground mt-0.5 truncate text-xs">“{{ r.quote }}”</p>
            </div>
          </label>
        </div>
        <div class="grid gap-1.5">
          <label for="merge-name" class="text-xs font-medium">Name after merging</label>
          <Input
            id="merge-name"
            v-model="name"
            autocomplete="off"
            placeholder="Name"
            :disabled="picked.length < 2"
            class="h-8"
            @input="nameEdited = true"
          />
          <p class="text-muted-foreground text-xs">
            <template v-if="picked.length >= 2">
              {{ turns }} turns will be labeled “{{ name.trim() || "…" }}”. The labels from before stay in the transcription history.
            </template>
            <template v-else>Tick at least two speakers.</template>
          </p>
        </div>
        <DialogFooter>
          <Button type="button" variant="ghost" @click="emit('close')">Cancel</Button>
          <Button type="submit" :disabled="!canMerge || saving">
            <Loader2 v-if="saving" class="animate-spin" /><Merge v-else />
            Merge{{ picked.length >= 2 ? ` ${picked.length} speakers` : "" }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
