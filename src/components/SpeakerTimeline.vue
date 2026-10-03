<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { useEventListener, useResizeObserver } from "@vueuse/core";
import { AlertTriangle, Maximize2, Pause, ZoomIn, ZoomOut } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import type { AudioLevels, SegmentRow } from "@/lib/api";
import { formatTime, speakerColor } from "@/lib/format";
import { findGaps, formatGap, gapKey, TimelineScale } from "@/lib/breaks";

const props = defineProps<{
  segments: SegmentRow[];
  duration: number;
  currentTime: number;
  playing: boolean;
  speakerOrder: string[];
  names: Record<string, string>;
  matches: Set<number>;
  /** Shrink long stretches without speech to a small marker. */
  collapse: boolean;
  /** Gaps (by `gapKey`) whose audio has speech: skipped text, not a pause. Never collapsed. */
  missing: Set<number>;
  /** Loudness over time; null while loading or unavailable. */
  levels: AudioLevels | null;
}>();
const emit = defineEmits<{ seek: [time: number] }>();

const LANE_H = 24;
const LEVEL_H = 28;
const ZOOM_MAX = 64;

const lanes = computed(() => {
  const hasUnknown = props.segments.some((s) => !s.speaker);
  return [...props.speakerOrder.map((l) => l as string | null), ...(hasUnknown ? [null] : [])];
});
const laneName = (l: string | null) => (l ? props.names[l] ?? l : "Unlabeled");
const total = computed(() => Math.max(props.duration, props.segments.at(-1)?.end ?? 0, 1));
const gaps = computed(() => findGaps(props.segments, total.value));
const pauses = computed(() => gaps.value.filter((g) => !props.missing.has(gapKey(g))));
const scale = computed(() => new TimelineScale(total.value, props.collapse ? pauses.value : []));
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
      })),
  })),
);

// --- zoom + scrolling --------------------------------------------------------

const zoom = ref(1);
const scroller = ref<HTMLElement | null>(null);
const track = ref<HTMLElement | null>(null);
const viewWidth = ref(0);
const scrollLeft = ref(0);
useResizeObserver(scroller, (entries) => (viewWidth.value = entries[0].contentRect.width));

/** Set the zoom level, keeping the time under `anchorX` (px from the left of the viewport) in place. */
function setZoom(z: number, anchorX?: number) {
  const el = scroller.value;
  const next = Math.min(ZOOM_MAX, Math.max(1, z));
  if (!el || next === zoom.value) {
    zoom.value = next;
    return;
  }
  const ax = anchorX ?? el.clientWidth / 2;
  const frac = (el.scrollLeft + ax) / Math.max(1, el.scrollWidth);
  zoom.value = next;
  void nextTick(() => {
    el.scrollLeft = frac * el.scrollWidth - ax;
    scrollLeft.value = el.scrollLeft;
  });
}
const zoomLabel = computed(() => (zoom.value < 10 ? zoom.value.toFixed(1).replace(/\.0$/, "") : Math.round(zoom.value)) + "×");

function onWheel(e: WheelEvent) {
  const el = scroller.value;
  if (!el) return;
  if (e.ctrlKey || e.metaKey) {
    // Pinch on a trackpad arrives as ctrl+wheel.
    e.preventDefault();
    setZoom(zoom.value * Math.exp(-e.deltaY * 0.01), e.clientX - el.getBoundingClientRect().left);
  } else if (zoom.value > 1 && Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
    e.preventDefault();
    el.scrollLeft += e.deltaY;
  }
}

/** Keep the playhead in view while playing zoomed in. */
watch(
  () => props.currentTime,
  (t) => {
    const el = scroller.value;
    if (!el || !props.playing || zoom.value <= 1) return;
    const x = scale.value.pos(t) * el.scrollWidth;
    if (x < el.scrollLeft || x > el.scrollLeft + el.clientWidth) el.scrollLeft = Math.max(0, x - el.clientWidth * 0.2);
  },
);

/** Zoom in on the stretch around a time (used by double-click). */
function zoomAround(time: number, factor: number) {
  const el = scroller.value;
  if (!el) return;
  setZoom(zoom.value * factor, scale.value.pos(time) * el.scrollWidth - el.scrollLeft);
}

// --- hover -------------------------------------------------------------------

const hover = ref<{ time: number; lane: number; x: number; y: number } | null>(null);
/** Speaker whose lane label is hovered: other lanes fade so their turns stand out. */
const focusLane = ref<string | null | undefined>(undefined);

function timeAt(e: MouseEvent) {
  const r = track.value!.getBoundingClientRect();
  return scale.value.time((e.clientX - r.left) / r.width);
}
function onMove(e: MouseEvent) {
  const r = track.value!.getBoundingClientRect();
  hover.value = {
    time: scale.value.time((e.clientX - r.left) / r.width),
    lane: Math.floor((e.clientY - r.top) / LANE_H),
    x: e.clientX,
    y: e.clientY,
  };
}

const hoverInfo = computed(() => {
  const h = hover.value;
  if (!h) return null;
  const hits = props.segments.filter((s) => s.start <= h.time && h.time <= s.end);
  const lane = lanes.value[h.lane];
  const seg = hits.find((s) => s.speaker === (lane ?? null)) ?? hits[0] ?? null;
  const lv = props.levels;
  const i = lv ? Math.floor(h.time / lv.step) : -1;
  const db = lv && i >= 0 && i < lv.levels.length ? Math.round((lv.levels[i] / 255) * 60 - 60) : null;
  const gap = gaps.value.find((g) => g.start <= h.time && h.time <= g.end);
  return { seg, db, gap, time: h.time, x: Math.min(h.x + 14, window.innerWidth - 300), y: h.y + 16 };
});

// --- waveform lane -------------------------------------------------------------

const canvas = ref<HTMLCanvasElement | null>(null);

/** Lane index per level bucket (-1 = nobody speaking), so the waveform is colored by speaker. */
const speakerAt = computed(() => {
  const lv = props.levels;
  if (!lv) return null;
  const out = new Int16Array(lv.levels.length).fill(-1);
  const index = new Map(lanes.value.map((l, i) => [l, i]));
  for (const s of props.segments) {
    const li = index.get(s.speaker) ?? -1;
    const a = Math.max(0, Math.floor(s.start / lv.step));
    const b = Math.min(out.length, Math.ceil(s.end / lv.step));
    for (let i = a; i < b; i++) if (out[i] < 0) out[i] = li;
  }
  return out;
});

let drawQueued = false;
function requestDraw() {
  if (drawQueued) return;
  drawQueued = true;
  requestAnimationFrame(() => {
    drawQueued = false;
    draw();
  });
}
function draw() {
  const c = canvas.value;
  const el = scroller.value;
  if (!c || !el) return;
  const w = el.clientWidth;
  const h = LEVEL_H;
  const dpr = window.devicePixelRatio || 1;
  c.width = Math.max(1, Math.round(w * dpr));
  c.height = Math.round(h * dpr);
  const ctx = c.getContext("2d");
  if (!ctx) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  const lv = props.levels;
  const who = speakerAt.value;
  if (!lv || !who) return;
  const sw = el.scrollWidth;
  const left = el.scrollLeft;
  const sc = scale.value;
  const colors = lanes.value.map((l) => speakerColor(l, props.speakerOrder));
  const silent = getComputedStyle(c).color;
  const played = props.currentTime;
  let t1 = sc.time(left / sw);
  for (let x = 0; x < w; x++) {
    const t0 = t1;
    t1 = sc.time((left + x + 1) / sw);
    const i0 = Math.floor(t0 / lv.step);
    const i1 = Math.max(i0 + 1, Math.ceil(t1 / lv.step));
    let max = 0;
    let lane = -1;
    for (let i = i0; i < i1 && i < lv.levels.length; i++) {
      if (lv.levels[i] > max) max = lv.levels[i];
      if (lane < 0 && who[i] >= 0) lane = who[i];
    }
    if (max <= 0) continue;
    const amp = Math.max(1, (max / 255) * h);
    ctx.globalAlpha = t0 <= played ? 0.95 : 0.55;
    ctx.fillStyle = lane >= 0 ? colors[lane] : silent;
    ctx.fillRect(x, (h - amp) / 2, 1, amp);
  }
}
watch([() => props.levels, speakerAt, scale, zoom, viewWidth, scrollLeft], requestDraw);
// Redraw the played/unplayed split on time updates (cheap: one pass over the visible pixels).
watch(() => props.currentTime, requestDraw);
onMounted(requestDraw);
useEventListener(scroller, "scroll", () => (scrollLeft.value = scroller.value?.scrollLeft ?? 0), { passive: true });
</script>

<template>
  <div class="select-none">
    <div class="text-muted-foreground mb-1 flex items-center justify-end gap-1 text-[11px]">
      <span class="mr-2 hidden sm:inline">⌘/Ctrl + scroll or double-click to zoom · drag the bar to pan</span>
      <Button variant="ghost" size="icon-xs" title="Zoom out" :disabled="zoom <= 1" @click="setZoom(zoom / 1.5)"><ZoomOut /></Button>
      <span class="w-9 text-center tabular-nums">{{ zoomLabel }}</span>
      <Button variant="ghost" size="icon-xs" title="Zoom in" :disabled="zoom >= ZOOM_MAX" @click="setZoom(zoom * 1.5)"><ZoomIn /></Button>
      <Button variant="ghost" size="icon-xs" title="Fit the whole recording" :disabled="zoom <= 1" @click="setZoom(1)"><Maximize2 /></Button>
    </div>
    <div class="flex">
      <div class="w-24 shrink-0" @mouseleave="focusLane = undefined">
        <div class="h-4" />
        <div
          v-for="l in lanes"
          :key="l ?? 'unknown'"
          class="flex h-6 cursor-default items-center gap-1.5 rounded-sm pr-2 text-xs"
          :class="focusLane === l && 'bg-accent'"
          :title="`Highlight ${laneName(l)}`"
          @mouseenter="focusLane = l"
        >
          <span class="size-2 shrink-0 rounded-full" :style="{ backgroundColor: speakerColor(l, speakerOrder) }" />
          <span class="truncate">{{ laneName(l) }}</span>
        </div>
        <div class="text-muted-foreground flex h-7 items-center text-[11px]" title="Loudness of the recording, colored by who is speaking">Level</div>
      </div>
      <div
        ref="scroller"
        class="timeline-scroller min-w-0 flex-1"
        :class="zoom > 1 ? 'overflow-x-auto' : 'overflow-x-hidden'"
        @wheel="onWheel"
      >
        <div :style="{ width: `${zoom * 100}%` }">
          <div class="text-muted-foreground relative h-4 text-[10px] tabular-nums">
            <span v-for="t in scale.ticks(zoom)" :key="t" class="absolute" :class="t > 0 && '-translate-x-1/2'" :style="{ left: pct(t) }">{{ formatTime(t) }}</span>
          </div>
          <div
            ref="track"
            class="bg-muted/40 relative cursor-pointer overflow-hidden rounded-md"
            @click="emit('seek', timeAt($event))"
            @dblclick.prevent="zoomAround(timeAt($event), 2)"
            @mousemove="onMove"
            @mouseleave="hover = null"
          >
            <div
              v-for="lane in bars"
              :key="lane.key"
              class="transition-opacity"
              :class="focusLane !== undefined && (focusLane ?? 'unknown') !== lane.key && 'opacity-15'"
            >
              <div v-memo="[lane]" class="relative h-6 border-b border-white/40">
                <div
                  v-for="b in lane.segs"
                  :key="b.id"
                  class="absolute top-1 bottom-1 rounded-sm"
                  :class="b.dim ? 'opacity-25' : 'opacity-90'"
                  :style="{ left: b.left, width: b.width, backgroundColor: lane.color }"
                />
              </div>
            </div>
            <!-- Loudness, drawn only for the visible stretch -->
            <div class="relative" :style="{ height: `${LEVEL_H}px` }">
              <canvas ref="canvas" class="text-muted-foreground absolute top-0 h-full" :style="{ left: `${scrollLeft}px`, width: `${viewWidth}px` }" />
            </div>
            <!-- Breaks -->
            <div
              v-for="g in gaps"
              :key="g.start"
              class="pointer-events-none absolute top-0 bottom-0 flex items-center justify-center overflow-hidden"
              :class="
                missing.has(gapKey(g))
                  ? 'border-x border-dashed border-amber-500/60 bg-amber-500/10'
                  : collapse
                    ? 'break-stripes border-x border-dashed border-slate-300 dark:border-slate-600'
                    : ''
              "
              :style="{ left: pct(g.start), width: span(g.start, g.end) }"
            >
              <span
                v-if="missing.has(gapKey(g))"
                class="bg-background/80 flex items-center gap-0.5 rounded px-1 text-[10px] whitespace-nowrap text-amber-700 tabular-nums dark:text-amber-500"
              ><AlertTriangle class="size-2.5" />{{ formatGap(g.end - g.start) }}</span>
              <span
                v-else
                class="text-muted-foreground bg-background/80 flex items-center gap-0.5 rounded px-1 text-[10px] whitespace-nowrap tabular-nums"
              ><Pause class="size-2.5" />{{ formatGap(g.end - g.start) }}</span>
            </div>
            <div class="bg-foreground pointer-events-none absolute top-0 bottom-0 w-px" :style="{ left: pct(currentTime) }" />
            <div
              v-if="hover"
              class="bg-foreground/30 pointer-events-none absolute top-0 bottom-0 w-px"
              :style="{ left: pct(hover.time) }"
            />
          </div>
          <div class="text-muted-foreground relative h-4 text-[10px] tabular-nums">
            <span v-if="hover" class="absolute -translate-x-1/2" :style="{ left: pct(hover.time) }">{{ formatTime(hover.time) }}</span>
          </div>
        </div>
      </div>
    </div>

    <!-- Hover details: who is speaking, what they said, how loud -->
    <Teleport to="body">
      <div
        v-if="hoverInfo"
        class="bg-popover text-popover-foreground pointer-events-none fixed z-50 w-72 rounded-lg border p-2.5 text-xs shadow-md"
        :style="{ left: `${hoverInfo.x}px`, top: `${hoverInfo.y}px` }"
      >
        <div class="flex items-center gap-2">
          <span class="font-medium tabular-nums">{{ formatTime(hoverInfo.time) }}</span>
          <template v-if="hoverInfo.seg">
            <span class="size-2 rounded-full" :style="{ backgroundColor: speakerColor(hoverInfo.seg.speaker, speakerOrder) }" />
            <span class="truncate font-medium">{{ laneName(hoverInfo.seg.speaker) }}</span>
            <span class="text-muted-foreground ml-auto shrink-0 tabular-nums">
              {{ formatTime(hoverInfo.seg.start) }}–{{ formatTime(hoverInfo.seg.end) }}
            </span>
          </template>
          <span v-else-if="hoverInfo.gap" class="text-muted-foreground">
            {{ missing.has(gapKey(hoverInfo.gap)) ? "Speech without text" : "Pause" }} · {{ formatGap(hoverInfo.gap.end - hoverInfo.gap.start) }}
          </span>
          <span v-else class="text-muted-foreground">No speech</span>
        </div>
        <p v-if="hoverInfo.seg" class="text-muted-foreground mt-1 line-clamp-3 leading-snug">{{ hoverInfo.seg.text }}</p>
        <div v-if="hoverInfo.db !== null" class="mt-1.5 flex items-center gap-2">
          <div class="bg-muted h-1.5 flex-1 overflow-hidden rounded-full">
            <div
              class="h-full rounded-full"
              :class="hoverInfo.db > -6 ? 'bg-red-500' : hoverInfo.db > -18 ? 'bg-amber-500' : 'bg-emerald-500'"
              :style="{ width: `${Math.max(0, (hoverInfo.db + 60) / 60) * 100}%` }"
            />
          </div>
          <span class="text-muted-foreground w-12 text-right tabular-nums">{{ hoverInfo.db }} dB</span>
        </div>
      </div>
    </Teleport>
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
.timeline-scroller {
  scrollbar-width: thin;
  scrollbar-color: color-mix(in oklab, currentColor 25%, transparent) transparent;
}
</style>
