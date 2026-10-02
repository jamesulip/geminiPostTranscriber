import type { SegmentRow } from "./api";

/** Speech pauses at least this long are treated as breaks. */
export const BREAK_SECONDS = 60;

export interface Gap {
  start: number;
  end: number;
}

/** Stretches without any speech, including before the first and after the last segment. */
export function findGaps(segments: SegmentRow[], duration: number, min = BREAK_SECONDS): Gap[] {
  const sorted = [...segments].sort((a, b) => a.start - b.start);
  const gaps: Gap[] = [];
  let cursor = 0;
  for (const s of sorted) {
    if (s.start - cursor >= min) gaps.push({ start: cursor, end: s.start });
    cursor = Math.max(cursor, s.end);
  }
  if (duration - cursor >= min) gaps.push({ start: cursor, end: duration });
  return gaps;
}

export function formatGap(seconds: number): string {
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${s}s`;
}

interface Range {
  start: number;
  end: number;
  /** Display length (seconds-equivalent). */
  len: number;
  collapsed: boolean;
}

/**
 * Piecewise mapping between real time and timeline position. Collapsed gaps get a
 * small fixed width; everything else keeps its true proportion.
 */
export class TimelineScale {
  readonly ranges: Range[];
  readonly total: number;

  constructor(duration: number, gaps: Gap[]) {
    const speech = Math.max(1, duration - gaps.reduce((a, g) => a + (g.end - g.start), 0));
    // Each collapsed gap takes ~6% of the track (all gaps together at most ~30%).
    const frac = Math.min(0.06, 0.3 / Math.max(1, gaps.length));
    const gapLen = (frac * speech) / (1 - frac * gaps.length);
    const ranges: Range[] = [];
    let cursor = 0;
    for (const g of gaps) {
      if (g.start > cursor) ranges.push({ start: cursor, end: g.start, len: g.start - cursor, collapsed: false });
      ranges.push({ start: g.start, end: g.end, len: Math.min(g.end - g.start, gapLen), collapsed: true });
      cursor = g.end;
    }
    if (duration > cursor) ranges.push({ start: cursor, end: duration, len: duration - cursor, collapsed: false });
    if (!ranges.length) ranges.push({ start: 0, end: Math.max(duration, 1), len: Math.max(duration, 1), collapsed: false });
    this.ranges = ranges;
    this.total = ranges.reduce((a, r) => a + r.len, 0);
  }

  /** Real time → 0..1 */
  pos(t: number): number {
    let offset = 0;
    for (const r of this.ranges) {
      if (t <= r.end) return (offset + ((Math.max(t, r.start) - r.start) / (r.end - r.start || 1)) * r.len) / this.total;
      offset += r.len;
    }
    return 1;
  }

  /** 0..1 → real time */
  time(p: number): number {
    let target = Math.min(Math.max(p, 0), 1) * this.total;
    for (const r of this.ranges) {
      if (target <= r.len) return r.start + (target / (r.len || 1)) * (r.end - r.start);
      target -= r.len;
    }
    return this.ranges.at(-1)!.end;
  }

  /** Nice tick times that fall outside collapsed gaps and don't crowd each other. */
  ticks(): number[] {
    const steps = [10, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200];
    const step = steps.find((s) => this.total / s <= 10) ?? 7200;
    const out: number[] = [];
    let last = -1;
    for (const r of this.ranges) {
      if (r.collapsed) continue;
      for (let t = Math.ceil(r.start / step) * step; t <= r.end; t += step) {
        const p = this.pos(t);
        if (last < 0 || p - last > 0.06) {
          out.push(t);
          last = p;
        }
      }
    }
    return out;
  }
}
