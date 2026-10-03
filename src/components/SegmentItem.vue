<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, watch } from "vue";
import { Textarea } from "@/components/ui/textarea";
import type { SegmentRow } from "@/lib/api";
import { formatTime } from "@/lib/format";
import { highlightHtml } from "@/lib/highlight";

// One transcript row. Kept as its own component with primitive props so that, in long
// transcripts (thousands of rows), only rows whose props change re-render.
const props = defineProps<{
  seg: SegmentRow;
  speaker: string;
  color: string;
  active: boolean;
  /** Highlight query; empty for rows that don't match (so they don't re-render). */
  query: string;
  readOnly: boolean;
  editing: boolean;
}>();
const emit = defineEmits<{
  seek: [time: number];
  speakerMenu: [seg: SegmentRow, anchor: HTMLElement];
  edit: [];
  save: [text: string];
  cancel: [];
}>();

const draft = ref("");
const box = ref<InstanceType<typeof Textarea> | null>(null);
watch(
  () => props.editing,
  (on) => {
    if (!on) return;
    draft.value = props.seg.text;
    void nextTick(() => (box.value?.$el as HTMLTextAreaElement | undefined)?.focus());
  },
  { immediate: true },
);
// The list is virtualized, so scrolling a row out of view unmounts it: keep an unsaved edit, and
// hand focus back to the page so keyboard shortcuts (space, arrows) keep working.
const root = ref<HTMLElement | null>(null);
onBeforeUnmount(() => {
  if (props.editing) emit("save", draft.value);
  if (root.value?.contains(document.activeElement)) {
    root.value.parentElement?.closest<HTMLElement>("[tabindex]")?.focus({ preventScroll: true });
  }
});
</script>

<template>
  <div
    ref="root"
    class="grid grid-cols-[64px_120px_1fr] gap-3 rounded-lg px-2 py-1.5"
    :class="active ? 'bg-primary/10' : 'hover:bg-accent/40'"
  >
    <button class="text-muted-foreground hover:text-primary pt-0.5 text-left text-xs tabular-nums" @click="emit('seek', seg.start)">
      {{ formatTime(seg.start) }}
    </button>
    <button
      class="flex h-fit items-center gap-1.5 truncate pt-0.5 text-left text-xs font-medium disabled:cursor-default"
      :disabled="readOnly"
      @click="emit('speakerMenu', seg, $event.currentTarget as HTMLElement)"
    >
      <span class="size-2 shrink-0 rounded-full" :style="{ backgroundColor: color }" />
      <span class="truncate">{{ speaker }}</span>
    </button>
    <Textarea
      v-if="editing"
      ref="box"
      v-model="draft"
      class="min-h-0 text-sm"
      @blur="emit('save', draft)"
      @keydown.enter.exact.prevent="emit('save', draft)"
      @keydown.esc="emit('cancel')"
    />
    <p
      v-else
      class="text-sm leading-relaxed"
      :class="readOnly ? 'cursor-auto' : 'cursor-text'"
      :title="readOnly ? '' : 'Double-click to edit'"
      @dblclick="!readOnly && emit('edit')"
      v-html="highlightHtml(seg.text, query)"
    />
  </div>
</template>
