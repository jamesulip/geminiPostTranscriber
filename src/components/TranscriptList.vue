<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { useVirtualizer } from "@tanstack/vue-virtual";
import { AlertTriangle, PauseCircle } from "@lucide/vue";
import SegmentItem from "@/components/SegmentItem.vue";
import type { SegmentRow } from "@/lib/api";
import { formatTime, speakerColor } from "@/lib/format";
import { BREAK_SECONDS, findGaps, formatGap, gapKey, type Gap } from "@/lib/breaks";

// The transcript rows, virtualized: only rows in (or near) the viewport are mounted, so
// transcripts with thousands of segments open instantly and stay smooth during playback.
// Its own component so the page re-rendering (e.g. on every audio timeupdate) skips it.
const props = defineProps<{
  segments: SegmentRow[];
  /** Insert "pause" dividers where speech stops for a while. */
  breaks: boolean;
  /** Length used for pause markers: not-yet-transcribed audio isn't a "break". */
  duration: number;
  /** Gaps (by `gapKey`) whose audio has speech: skipped text, not a pause. */
  missing: Set<number>;
  activeId: number | null;
  /** Highlight query, applied to the segments in `matches`. */
  query: string;
  matches: Set<number>;
  speakerOrder: string[];
  names: Record<string, string>;
  readOnly: boolean;
  editingId: number | null;
}>();
const emit = defineEmits<{
  seek: [time: number];
  speakerMenu: [seg: SegmentRow, anchor: HTMLElement];
  edit: [seg: SegmentRow];
  save: [seg: SegmentRow, text: string];
  cancel: [];
}>();

const speakerName = (label: string | null) => (label ? props.names[label] ?? label : "Unlabeled");

type Row = { kind: "seg"; seg: SegmentRow } | { kind: "break"; gap: Gap };
const rows = computed<Row[]>(() => {
  const segs = props.segments;
  if (!props.breaks) return segs.map((seg) => ({ kind: "seg", seg }));
  const gaps = findGaps(segs, props.duration, BREAK_SECONDS);
  const out: Row[] = [];
  let g = 0;
  for (const seg of segs) {
    while (g < gaps.length && gaps[g].end <= seg.start + 1e-6) out.push({ kind: "break", gap: gaps[g++] });
    out.push({ kind: "seg", seg });
  }
  while (g < gaps.length) out.push({ kind: "break", gap: gaps[g++] });
  return out;
});
const rowKey = (r: Row) => (r.kind === "seg" ? r.seg.id : `gap-${r.gap.start}`);

const scroller = ref<HTMLElement | null>(null);
const virtualizer = useVirtualizer(
  computed(() => ({
    count: rows.value.length,
    getScrollElement: () => scroller.value,
    // Keyed by row so measured heights survive filtering and reloads.
    getItemKey: (i: number) => rowKey(rows.value[i]),
    estimateSize: () => 36,
    overscan: 10,
    paddingStart: 12,
    paddingEnd: 12,
  })),
);
const items = computed(() => virtualizer.value.getVirtualItems().map((item) => ({ item, row: rows.value[item.index] })));

/** Scroll a segment to the middle of the list (no-op if it's hidden, e.g. by "only matches"). */
async function scrollToSegment(id: number, smooth = false) {
  await nextTick();
  const i = rows.value.findIndex((r) => r.kind === "seg" && r.seg.id === id);
  if (i >= 0) virtualizer.value.scrollToIndex(i, { align: "center", behavior: smooth ? "smooth" : "auto" });
}
defineExpose({ scrollToSegment });
</script>

<template>
  <div ref="scroller" class="min-h-0 flex-1 overflow-y-auto px-6">
    <div class="relative w-full" :style="{ height: `${virtualizer.getTotalSize()}px` }">
      <div class="absolute inset-x-0 top-0" :style="{ transform: `translateY(${items[0]?.item.start ?? 0}px)` }">
        <!-- Each row is measured as a whole, so spacing must be padding (margins aren't measured). -->
        <div
          v-for="{ item, row } in items"
          :key="item.key as PropertyKey"
          :ref="(el) => virtualizer.measureElement(el as Element | null)"
          :data-index="item.index"
        >
          <div
            v-if="row.kind === 'break' && missing.has(gapKey(row.gap))"
            class="flex items-center gap-3 px-2 py-2 text-xs text-amber-700 dark:text-amber-500"
          >
            <span class="h-px flex-1 border-t border-dashed border-current" />
            <button
              class="flex items-center gap-1 tabular-nums hover:underline"
              title="There's speech here but no text. Use Fix problems to transcribe it."
              @click="emit('seek', row.gap.start)"
            >
              <AlertTriangle class="size-3.5" /> {{ formatGap(row.gap.end - row.gap.start) }} not transcribed · {{ formatTime(row.gap.start) }} – {{ formatTime(row.gap.end) }}
            </button>
            <span class="h-px flex-1 border-t border-dashed border-current" />
          </div>
          <div v-else-if="row.kind === 'break'" class="text-muted-foreground flex items-center gap-3 px-2 py-2 text-xs">
            <span class="h-px flex-1 border-t border-dashed" />
            <button class="hover:text-primary flex items-center gap-1 tabular-nums" title="Jump to the end of the pause" @click="emit('seek', row.gap.end)">
              <PauseCircle class="size-3.5" /> {{ formatGap(row.gap.end - row.gap.start) }} pause · {{ formatTime(row.gap.start) }} – {{ formatTime(row.gap.end) }}
            </button>
            <span class="h-px flex-1 border-t border-dashed" />
          </div>
          <SegmentItem
            v-else
            :seg="row.seg"
            :speaker="speakerName(row.seg.speaker)"
            :color="speakerColor(row.seg.speaker, speakerOrder)"
            :active="row.seg.id === activeId"
            :query="matches.has(row.seg.id) ? query : ''"
            :read-only="readOnly"
            :editing="editingId === row.seg.id"
            @seek="(time) => emit('seek', time)"
            @speaker-menu="(seg, anchor) => emit('speakerMenu', seg, anchor)"
            @edit="emit('edit', row.seg)"
            @save="(text) => emit('save', row.seg, text)"
            @cancel="emit('cancel')"
          />
        </div>
      </div>
    </div>
  </div>
</template>
