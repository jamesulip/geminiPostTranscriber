import type { SegmentRow } from "./api";

// Word-level diff between two transcriptions of the same recording ("git diff" for versions).
//
// Versions from different models split speech into different segments, so lines can't be
// compared 1:1. Instead both sides are flattened into words, aligned with a longest common
// subsequence in which words may only match when they're near each other in time (which keeps
// it fast and stops common words from matching across the recording), and then cut back into
// blocks of whole segments wherever no aligned word crosses the cut.

export interface DiffSide {
  segments: SegmentRow[];
  /** Speaker label → display name. */
  names: Record<string, string>;
}

export interface DiffOptions {
  /** Compare words ignoring case and punctuation. */
  loose: boolean;
  /** Also flag words whose speaker differs. */
  speakers: boolean;
  /** Words only match when their (estimated) times are at most this many seconds apart. */
  window: number;
}

/** Versions from different models can disagree on timestamps by a couple of minutes. */
export const DEFAULT_WINDOW = 180;

/** A run of words that are all changed or all unchanged. */
export interface DiffPart {
  text: string;
  changed: boolean;
}

export interface DiffLine {
  seg: SegmentRow;
  speaker: string;
  speakerChanged: boolean;
  parts: DiffPart[];
}

/** Whole segments on both sides that only align with each other. */
export interface DiffBlock {
  changed: boolean;
  start: number;
  end: number;
  a: DiffLine[];
  b: DiffLine[];
  removed: number;
  added: number;
}

export interface DiffResult {
  blocks: DiffBlock[];
  /** Compared words only in A / only in B / in both. */
  removed: number;
  added: number;
  same: number;
  /** 0..1 share of words in common. */
  similarity: number;
  /** Aligned words whose speaker name differs (counted even when `speakers` is off). */
  speakerChanges: number;
}

interface Token {
  seg: number;
  word: number;
  norm: string;
  t: number;
}

interface Tokenized {
  segs: SegmentRow[];
  words: string[][];
  tokens: Token[];
  /** Index of each segment's first token; one extra entry = token count. */
  first: number[];
}

const PUNCT = /[\p{P}\p{S}]/gu;

function tokenize(segments: SegmentRow[], loose: boolean): Tokenized {
  const segs = [...segments].sort((x, y) => x.start - y.start);
  const words: string[][] = [];
  const tokens: Token[] = [];
  const first: number[] = [];
  let prevT = 0;
  segs.forEach((s, si) => {
    const ws = s.text.split(/\s+/).filter(Boolean);
    words.push(ws);
    first.push(tokens.length);
    ws.forEach((w, wi) => {
      const norm = loose ? w.toLowerCase().replace(PUNCT, "") : w;
      if (!norm) return; // punctuation-only: shown, but not compared
      // Spread words evenly over the segment; keep times non-decreasing for the time window.
      prevT = Math.max(prevT, s.start + ((s.end - s.start) * (wi + 0.5)) / ws.length);
      tokens.push({ seg: si, word: wi, norm, t: prevT });
    });
  });
  first.push(tokens.length);
  return { segs, words, tokens, first };
}

/** First index in [lo, hi) where `pred` is true (pred must be monotone false→true). */
function search(lo: number, hi: number, pred: (i: number) => boolean): number {
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (pred(mid)) hi = mid;
    else lo = mid + 1;
  }
  return lo;
}

/**
 * Longest common subsequence of `a` and `b`, only matching equal words within `window` seconds
 * (Hunt–Szymanski: work grows with the number of candidate matches, not with |a|·|b|).
 * Returns, for each token of `a`, the index of its match in `b` or -1.
 */
function align(a: Token[], b: Token[], window: number): Int32Array {
  const byWord = new Map<string, number[]>();
  b.forEach((tok, j) => {
    const list = byWord.get(tok.norm);
    if (list) list.push(j);
    else byWord.set(tok.norm, [j]);
  });
  type Node = { i: number; j: number; prev: Node | null };
  const ends: number[] = []; // ends[k]: smallest b index ending a common subsequence of length k + 1
  const nodes: Node[] = [];
  for (let i = 0; i < a.length; i++) {
    const list = byWord.get(a[i].norm);
    if (!list) continue;
    const t = a[i].t;
    const from = search(0, list.length, (x) => b[list[x]].t >= t - window);
    const to = search(from, list.length, (x) => b[list[x]].t > t + window);
    // Descending, so one `a` word can't extend a chain it just ended.
    for (let x = to - 1; x >= from; x--) {
      const j = list[x];
      const k = search(0, ends.length, (y) => ends[y] >= j);
      ends[k] = j;
      nodes[k] = { i, j, prev: k > 0 ? nodes[k - 1] : null };
    }
  }
  const match = new Int32Array(a.length).fill(-1);
  for (let n: Node | null = nodes[ends.length - 1] ?? null; n; n = n.prev) match[n.i] = n.j;
  return match;
}

/**
 * Possible cuts before each segment boundary of `x`: the boundary of `y` closest in time
 * such that no aligned word crosses the cut.
 */
function cutsFrom(x: Tokenized, y: Tokenized, match: Int32Array, swap: boolean): [number, number][] {
  const nx = x.tokens.length;
  const ny = y.tokens.length;
  // Last matched y-token before x-token p, and first matched y-token at or after p.
  const prev = new Int32Array(nx + 1);
  const next = new Int32Array(nx + 1);
  prev[0] = -1;
  for (let p = 0; p < nx; p++) prev[p + 1] = match[p] >= 0 ? match[p] : prev[p];
  next[nx] = ny;
  for (let p = nx - 1; p >= 0; p--) next[p] = match[p] >= 0 ? match[p] : next[p + 1];

  const ySegs = y.segs.length;
  const yStart = (s: number) => (s < ySegs ? y.segs[s].start : Infinity);
  const out: [number, number][] = [];
  for (let s = 1; s < x.segs.length; s++) {
    const p = x.first[s];
    // y boundaries s' (0..ySegs) whose first token lies in (prev, next].
    const lo = search(0, ySegs + 1, (q) => y.first[q] > prev[p]);
    const hi = search(lo, ySegs + 1, (q) => y.first[q] > next[p]) - 1;
    if (lo > hi) continue;
    const t = x.segs[s].start;
    let best = search(lo, hi + 1, (q) => yStart(q) >= t);
    if (best > hi || (best > lo && t - yStart(best - 1) < yStart(best) - t)) best -= 1;
    out.push(swap ? [best, s] : [s, best]);
  }
  return out;
}

function lineFor(tk: Tokenized, s: number, matched: (token: number) => boolean, names: Record<string, string>): DiffLine {
  const seg = tk.segs[s];
  const words = tk.words[s];
  const changed = new Array<boolean>(words.length).fill(false);
  for (let k = tk.first[s]; k < tk.first[s + 1]; k++) if (!matched(k)) changed[tk.tokens[k].word] = true;
  const parts: DiffPart[] = [];
  words.forEach((w, i) => {
    const last = parts.at(-1);
    if (last && last.changed === changed[i]) last.text += ` ${w}`;
    else parts.push({ text: w, changed: changed[i] });
  });
  return { seg, speaker: speakerName(seg.speaker, names), speakerChanged: false, parts };
}

function speakerName(label: string | null, names: Record<string, string>): string {
  return label ? names[label] ?? label : "";
}

export function diffTranscripts(a: DiffSide, b: DiffSide, opts: DiffOptions): DiffResult {
  const ta = tokenize(a.segments, opts.loose);
  const tb = tokenize(b.segments, opts.loose);
  const matchA = align(ta.tokens, tb.tokens, opts.window);
  const matchB = new Int32Array(tb.tokens.length).fill(-1);
  matchA.forEach((j, i) => j >= 0 && (matchB[j] = i));

  // Cut points from both sides, merged into one non-crossing sequence.
  const candidates = [
    ...cutsFrom(ta, tb, matchA, false),
    ...cutsFrom(tb, ta, matchB, true),
    [ta.segs.length, tb.segs.length] as [number, number],
  ].sort((x, y) => x[0] - y[0] || x[1] - y[1]);
  const cuts: [number, number][] = [[0, 0]];
  for (const c of candidates) {
    const [la, lb] = cuts.at(-1)!;
    if (c[0] >= la && c[1] >= lb && (c[0] > la || c[1] > lb)) cuts.push(c);
  }

  let speakerChanges = 0;
  const blocks: DiffBlock[] = [];
  for (let n = 1; n < cuts.length; n++) {
    const [a0, b0] = cuts[n - 1];
    const [a1, b1] = cuts[n];
    const linesA = [];
    const linesB = [];
    for (let s = a0; s < a1; s++) linesA.push(lineFor(ta, s, (k) => matchA[k] >= 0, a.names));
    for (let s = b0; s < b1; s++) linesB.push(lineFor(tb, s, (k) => matchB[k] >= 0, b.names));
    let removed = 0;
    let added = 0;
    for (let k = ta.first[a0]; k < ta.first[a1]; k++) {
      const j = matchA[k];
      if (j < 0) {
        removed++;
        continue;
      }
      const sa = ta.tokens[k].seg;
      const sb = tb.tokens[j].seg;
      if (speakerName(ta.segs[sa].speaker, a.names) !== speakerName(tb.segs[sb].speaker, b.names)) {
        speakerChanges++;
        if (opts.speakers) {
          linesA[sa - a0].speakerChanged = true;
          linesB[sb - b0].speakerChanged = true;
        }
      }
    }
    for (let k = tb.first[b0]; k < tb.first[b1]; k++) if (matchB[k] < 0) added++;
    const lines = [...linesA, ...linesB];
    blocks.push({
      changed: removed > 0 || added > 0 || lines.some((l) => l.speakerChanged),
      start: Math.min(...lines.map((l) => l.seg.start)),
      end: Math.max(...lines.map((l) => l.seg.end)),
      a: linesA,
      b: linesB,
      removed,
      added,
    });
  }

  const removed = blocks.reduce((x, bl) => x + bl.removed, 0);
  const added = blocks.reduce((x, bl) => x + bl.added, 0);
  const same = ta.tokens.length - removed;
  const total = ta.tokens.length + tb.tokens.length;
  return { blocks, removed, added, same, similarity: total ? (2 * same) / total : 1, speakerChanges };
}

// --- display rows -------------------------------------------------------------

export type DiffRow =
  | { kind: "hunk"; key: string; n: number; start: number; end: number; removed: number; added: number }
  | { kind: "line"; key: string; sign: "-" | "+" | " "; line: DiffLine }
  | { kind: "skip"; key: string; from: number; segments: number; start: number; end: number };

/**
 * Git-style rows: one hunk per changed block (its − lines, then its + lines) with `context`
 * unchanged blocks around it. The rest of an unchanged stretch is collapsed into a "skip" row
 * (if it's at least 3 blocks) unless its first block index is in `expanded`. Blocks are small
 * (usually a few seconds), so hunks stay readable even when nearly everything differs, e.g.
 * between two different models.
 */
export function diffRows(blocks: DiffBlock[], expanded: Set<number>, context = 1): { rows: DiffRow[]; hunks: number[] } {
  const rows: DiffRow[] = [];
  const hunks: number[] = [];
  const pushContext = (i: number) =>
    blocks[i].b.forEach((line, k) => rows.push({ kind: "line", key: `c${i}.${k}`, sign: " ", line }));
  let i = 0;
  while (i < blocks.length) {
    const bl = blocks[i];
    if (bl.changed) {
      hunks.push(rows.length);
      rows.push({ kind: "hunk", key: `h${i}`, n: hunks.length, start: bl.start, end: bl.end, removed: bl.removed, added: bl.added });
      bl.a.forEach((line, m) => rows.push({ kind: "line", key: `a${i}.${m}`, sign: "-", line }));
      bl.b.forEach((line, m) => rows.push({ kind: "line", key: `b${i}.${m}`, sign: "+", line }));
      i++;
      continue;
    }
    let j = i;
    while (j < blocks.length && !blocks[j].changed) j++;
    // Unchanged run [i, j): keep `context` blocks next to hunks, collapse the middle.
    const head = i === 0 ? 0 : context;
    const tail = j === blocks.length ? 0 : context;
    if (j - i - head - tail < 3 || expanded.has(i)) {
      for (let k = i; k < j; k++) pushContext(k);
    } else {
      for (let k = i; k < i + head; k++) pushContext(k);
      const hidden = blocks.slice(i + head, j - tail);
      rows.push({
        kind: "skip",
        key: `s${i}`,
        from: i,
        segments: hidden.reduce((x, bl) => x + bl.b.length, 0),
        start: hidden[0].start,
        end: hidden.at(-1)!.end,
      });
      for (let k = j - tail; k < j; k++) pushContext(k);
    }
    i = j;
  }
  return { rows, hunks };
}
