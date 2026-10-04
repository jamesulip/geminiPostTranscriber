/** Tiny markdown renderer for assistant output: headings, lists, bold/italic/code, paragraphs,
 *  and `[[t:<transcript id>:<seconds>]]` citation tokens rendered as clickable time chips. */
import { formatTime } from "./format";

const CITE = /\[\[t:([0-9A-Za-z_-]+):([0-9]+(?:\.[0-9]+)?)\]\]/g;
/** Grouped form some models produce: `[[t:a:1], [t:b:2]]` → separate tokens. */
const GROUPED = /\[\[(t:[^[\]]+(?:\]\s*,\s*\[t:[^[\]]+)+)\]\]/g;
function normalizeCitations(s: string): string {
  return s.replace(GROUPED, (_m, inner: string) => inner.split("],").map((p) => `[[${p.trim().replace(/^\[/, "").replace(/\]$/, "")}]]`).join(" "));
}

export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Citation chip. `currentId` chips show just the time; others show the meeting title too. */
function cite(id: string, secs: string, currentId: string | undefined, titles: Record<string, string>): string {
  const t = parseFloat(secs);
  const label = id === currentId ? formatTime(t) : `${titles[id] ?? "meeting"} · ${formatTime(t)}`;
  return `<button type="button" class="cite" data-cite-id="${escapeHtml(id)}" data-cite-time="${t}" title="Jump to ${formatTime(t)}">${escapeHtml(label)}</button>`;
}

function inline(s: string, currentId: string | undefined, titles: Record<string, string>): string {
  let out = escapeHtml(s);
  out = out.replace(/`([^`]+)`/g, "<code>$1</code>");
  out = out.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  out = out.replace(/(^|[\s(])\*([^*\n]+)\*(?=[\s).,;:!?]|$)/g, "$1<em>$2</em>");
  out = out.replace(CITE, (_m, id, secs) => cite(id, secs, currentId, titles));
  return out;
}

export function renderMarkdown(md: string, currentId?: string, titles: Record<string, string> = {}): string {
  const lines = normalizeCitations(md).replace(/\r/g, "").split("\n");
  const html: string[] = [];
  let list: "ul" | "ol" | null = null;
  let para: string[] = [];
  const flushPara = () => {
    if (para.length) html.push(`<p>${inline(para.join(" "), currentId, titles)}</p>`);
    para = [];
  };
  const closeList = () => {
    if (list) html.push(`</${list}>`);
    list = null;
  };
  for (const raw of lines) {
    const line = raw.trimEnd();
    const h = /^(#{1,4})\s+(.*)$/.exec(line);
    const ul = /^\s*[-*•]\s+(.*)$/.exec(line);
    const ol = /^\s*\d+[.)]\s+(.*)$/.exec(line);
    if (!line.trim()) {
      flushPara();
      closeList();
    } else if (h) {
      flushPara();
      closeList();
      const level = Math.min(h[1].length + 2, 5);
      html.push(`<h${level}>${inline(h[2], currentId, titles)}</h${level}>`);
    } else if (ul || ol) {
      flushPara();
      const kind = ul ? "ul" : "ol";
      if (list !== kind) {
        closeList();
        list = kind;
        html.push(`<${kind}>`);
      }
      html.push(`<li>${inline((ul ?? ol)![1], currentId, titles)}</li>`);
    } else {
      if (list && /^\s{2,}/.test(raw)) {
        // Continuation of the previous list item.
        html[html.length - 1] = html[html.length - 1].replace(/<\/li>$/, ` ${inline(line.trim(), currentId, titles)}</li>`);
      } else {
        closeList();
        para.push(line.trim());
      }
    }
  }
  flushPara();
  closeList();
  return html.join("\n");
}

/** Strip citation tokens for plain-text uses (copy, titles). */
export function stripCitations(md: string): string {
  return normalizeCitations(md).replace(CITE, "").replace(/[ \t]+([.,;:!?])/g, "$1").replace(/[ \t]{2,}/g, " ");
}

/** Resolve a click inside rendered markdown to a citation, if one was clicked. */
export function citationFromClick(e: MouseEvent): { id: string; time: number } | null {
  const el = (e.target as HTMLElement | null)?.closest<HTMLElement>("button.cite");
  if (!el) return null;
  return { id: el.dataset.citeId ?? "", time: parseFloat(el.dataset.citeTime ?? "0") };
}
