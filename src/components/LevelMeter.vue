<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { AudioLevels } from "@/lib/api";

/**
 * A small VU-style meter. Reads the precomputed loudness at the player's position (no Web Audio
 * needed, which the asset protocol doesn't allow anyway) and animates while playing.
 */
const props = defineProps<{
  levels: AudioLevels | null;
  audio: HTMLAudioElement | null;
  currentTime: number;
  playing: boolean;
}>();

const BARS = 14;
const shown = ref(0); // 0..1, with a quick attack and slow release
const peak = ref(0);
let peakAt = 0;
let raf = 0;

function levelAt(t: number): number {
  const lv = props.levels;
  if (!lv) return 0;
  const i = Math.floor(t / lv.step);
  return i >= 0 && i < lv.levels.length ? lv.levels[i] / 255 : 0;
}

function tick() {
  const now = performance.now();
  const t = props.audio?.currentTime ?? props.currentTime;
  const v = levelAt(t);
  shown.value = v > shown.value ? v : Math.max(v, shown.value - 0.04);
  if (v >= peak.value || now - peakAt > 1200) {
    peak.value = v;
    peakAt = now;
  }
  if (props.playing) raf = requestAnimationFrame(tick);
}

watch(
  () => props.playing,
  (on) => {
    cancelAnimationFrame(raf);
    if (on) raf = requestAnimationFrame(tick);
  },
  { immediate: true },
);
// When paused, show the level at the seek position.
watch(
  () => props.currentTime,
  (t) => {
    if (!props.playing) shown.value = peak.value = levelAt(t);
  },
  { immediate: true },
);
onBeforeUnmount(() => cancelAnimationFrame(raf));

const lit = computed(() => Math.round(shown.value * BARS));
const peakBar = computed(() => Math.round(peak.value * BARS));
const db = computed(() => Math.round(shown.value * 60 - 60));
const barClass = (i: number) => (i >= BARS - 2 ? "bg-red-500" : i >= BARS - 5 ? "bg-amber-500" : "bg-emerald-500");
</script>

<template>
  <div
    class="flex items-end gap-px"
    :title="levels ? `Level: ${db} dBFS` : 'Analyzing audio levels…'"
    :class="!levels && 'opacity-40'"
  >
    <div
      v-for="i in BARS"
      :key="i"
      class="w-1 rounded-[1px] transition-colors"
      :class="i <= lit || i === peakBar ? barClass(i - 1) : 'bg-muted-foreground/20'"
      :style="{ height: `${6 + (i / BARS) * 8}px` }"
    />
  </div>
</template>
