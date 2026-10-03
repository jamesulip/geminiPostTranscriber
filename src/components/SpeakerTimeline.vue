<script setup lang="ts">
import { computed, ref } from "vue";
import { Pause } from "@lucide/vue";
import type { SegmentRow } from "@/lib/api";
import { formatTime, speakerColor } from "@/lib/format";
import { findGaps, formatGap, TimelineScale } from "@/lib/breaks";

const props = defineProps<{
  segments: SegmentRow[];
  duration: number;
  currentTime: number;
  speakerOrder: string[];
  names: Record<string, string>;
  matches: Set<number>;
  /** Shrink long stretches without speech to a small marker. */
  collapse: boolean;
}>();
const emit = defineEmits<{ seek: [time: number] }>();

const lanes = computed(() => {
  const hasUnknown = props.segments.some((s) => !s.speaker);
  return [...props.speakerOrder.map((l) => l as string | null), ...(hasUnknown ? [null] : [])];
});
const total = computed(() => Math.max(props.duration, props.segments.at(-1)?.end ?? 0, 1));
const gaps = computed(() => findGaps(props.segments, total.value));
const scale = computed(() => new TimelineScale(total.value, props.collapse ? gaps.value : []));
const pct = (t: number) => `${scale.value.pos(t) * 100}%`;
const span = (a: number, b: number) => `${(scale.value.pos(b) - scale.value.pos(a)) * 100}%`;

/**
 * Segment bars per lane, computed once per transcript/scale/search. Rendered under v-memo so
 * playhead and hover updates don't re-render thousands of bars in long recordings.
 */
const bars = computed(() =>
  lanes.value.map((l) => ({
    key: l ?? "unknown",
    color: speakerColor(l, props.speakerOrder),
    segs: props.segments
      .filter((s) => s.speaker === l)
      .map((s) => ({
        id: s.id,
        left: pct(s.start),
        width: `max(2px, ${span(s.start, s.end)})`,
        dim: props.matches.size > 0 && !props.matches.has(s.id),
        title: `${formatTime(s.start)} ${s.text}`,
      })),
  })),
);

const track = ref<HTMLElement | null>(null);
const hover = ref<number | null>(null);

function timeAt(e: MouseEvent) {
  const r = track.value!.getBoundingClientRect();
  return scale.value.time((e.clientX - r.left) / r.width);
}
</script>

<template>
  <div class="select-none">
    <div class="flex">
      <div class="w-24 shrink-0" />
      <div class="text-muted-foreground relative h-4 flex-1 text-[10px] tabular-nums">
        <span v-for="t in scale.ticks()" :key="t" class="absolute -translate-x-1/2" :style="{ left: pct(t) }">{{ formatTime(t) }}</span>
      </div>
    </div>
    <div class="flex">
      <div class="w-24 shrink-0">
        <div v-for="l in lanes" :key="l ?? 'unknown'" class="flex h-6 items-center gap-1.5 pr-2 text-xs">
          <span class="size-2 shrink-0 rounded-full" :style="{ backgroundColor: speakerColor(l, speakerOrder) }" />
          <span class="truncate">{{ l ? names[l] ?? l : "Unlabeled" }}</span>
        </div>
      </div>
      <div
        ref="track"
        class="bg-muted/40 relative flex-1 cursor-pointer overflow-hidden rounded-md"
        @click="emit('seek', timeAt($event))"
        @mousemove="hover = timeAt($event)"
        @mouseleave="hover = null"
      >
        <div
          v-for="lane in bars"
          :key="lane.key"
          v-memo="[lane]"
          class="relative h-6 border-b border-white/40 last:border-0"
        >
          <div
            v-for="b in lane.segs"
            :key="b.id"
            class="absolute top-1 bottom-1 rounded-sm"
            :class="b.dim ? 'opacity-25' : 'opacity-90'"
            :style="{ left: b.left, width: b.width, backgroundColor: lane.color }"
            :title="b.title"
          />
        </div>
        <!-- Breaks -->
        <div
          v-for="g in gaps"
          :key="g.start"
          class="pointer-events-none absolute top-0 bottom-0 flex items-center justify-center overflow-hidden"
          :class="collapse ? 'break-stripes border-x border-dashed border-slate-300 dark:border-slate-600' : ''"
          :style="{ left: pct(g.start), width: span(g.start, g.end) }"
        >
          <span
            class="text-muted-foreground bg-background/80 flex items-center gap-0.5 rounded px-1 text-[10px] whitespace-nowrap tabular-nums"
            :title="`No speech for ${formatGap(g.end - g.start)}`"
          ><Pause class="size-2.5" />{{ formatGap(g.end - g.start) }}</span>
        </div>
        <div class="bg-foreground pointer-events-none absolute top-0 bottom-0 w-px" :style="{ left: pct(currentTime) }" />
        <div
          v-if="hover !== null"
          class="bg-foreground/30 pointer-events-none absolute top-0 bottom-0 w-px"
          :style="{ left: pct(hover) }"
        />
      </div>
    </div>
    <div class="flex">
      <div class="w-24 shrink-0" />
      <div class="text-muted-foreground relative h-4 flex-1 text-[10px] tabular-nums">
        <span v-if="hover !== null" class="absolute -translate-x-1/2" :style="{ left: pct(hover) }">{{ formatTime(hover) }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.break-stripes {
  background-image: repeating-linear-gradient(
    -45deg,
    color-mix(in oklab, currentColor 10%, transparent) 0 4px,
    transparent 4px 8px
  );
}
</style>
