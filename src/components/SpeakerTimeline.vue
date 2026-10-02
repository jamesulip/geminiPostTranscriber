<script setup lang="ts">
import { computed, ref } from "vue";
import type { SegmentRow } from "@/lib/api";
import { formatTime, speakerColor } from "@/lib/format";

const props = defineProps<{
  segments: SegmentRow[];
  duration: number;
  currentTime: number;
  speakerOrder: string[];
  names: Record<string, string>;
  matches: Set<number>;
}>();
const emit = defineEmits<{ seek: [time: number] }>();

const lanes = computed(() => {
  const labels = props.speakerOrder.length ? props.speakerOrder : [];
  const hasUnknown = props.segments.some((s) => !s.speaker);
  return [...labels.map((l) => l as string | null), ...(hasUnknown ? [null] : [])];
});
const total = computed(() => Math.max(props.duration, props.segments.at(-1)?.end ?? 0, 1));
const pct = (t: number) => `${(t / total.value) * 100}%`;

const track = ref<HTMLElement | null>(null);
const hover = ref<number | null>(null);

function timeAt(e: MouseEvent) {
  const r = track.value!.getBoundingClientRect();
  return Math.min(Math.max(0, (e.clientX - r.left) / r.width), 1) * total.value;
}

const ticks = computed(() => {
  const steps = [10, 30, 60, 120, 300, 600, 900, 1800, 3600];
  const step = steps.find((s) => total.value / s <= 10) ?? 3600;
  const out: number[] = [];
  for (let t = 0; t <= total.value; t += step) out.push(t);
  return out;
});
</script>

<template>
  <div class="select-none">
    <div class="flex">
      <div class="w-24 shrink-0" />
      <div class="text-muted-foreground relative h-4 flex-1 text-[10px] tabular-nums">
        <span v-for="t in ticks" :key="t" class="absolute -translate-x-1/2" :style="{ left: pct(t) }">{{ formatTime(t) }}</span>
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
        <div v-for="l in lanes" :key="l ?? 'unknown'" class="relative h-6 border-b border-white/40 last:border-0">
          <div
            v-for="s in segments.filter((x) => x.speaker === l)"
            :key="s.id"
            class="absolute top-1 bottom-1 rounded-sm"
            :class="matches.size && !matches.has(s.id) ? 'opacity-25' : 'opacity-90'"
            :style="{ left: pct(s.start), width: `max(2px, ${pct(s.end - s.start)})`, backgroundColor: speakerColor(l, speakerOrder) }"
            :title="`${formatTime(s.start)} ${s.text}`"
          />
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
