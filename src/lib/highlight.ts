function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

/** Render an FTS snippet (with <mark> markers) safely. */
export function snippetHtml(snippet: string): string {
  return escapeHtml(snippet).replace(/&lt;mark&gt;/g, "<mark>").replace(/&lt;\/mark&gt;/g, "</mark>");
}

/** Highlight occurrences of `query` words in plain text. */
export function highlightHtml(text: string, query: string): string {
  const words = query.trim().split(/\s+/).filter(Boolean).map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const safe = escapeHtml(text);
  if (!words.length) return safe;
  return safe.replace(new RegExp(`(${words.map(escapeHtml).join("|")})`, "gi"), "<mark>$1</mark>");
}
