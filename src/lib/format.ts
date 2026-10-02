export function formatTime(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

export function formatDuration(seconds: number): string {
  if (!seconds) return "—";
  const m = Math.round(seconds / 60);
  if (m < 1) return `${Math.round(seconds)}s`;
  return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m}m`;
}

export function formatDate(iso: string): string {
  return new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

const SPEAKER_COLORS = ["#3b82f6", "#f97316", "#10b981", "#a855f7", "#ef4444", "#eab308", "#06b6d4", "#ec4899"];

/** Stable color per speaker label, ordered by first appearance. */
export function speakerColor(label: string | null, order: string[]): string {
  if (!label) return "#94a3b8";
  const i = order.indexOf(label);
  return SPEAKER_COLORS[(i < 0 ? 0 : i) % SPEAKER_COLORS.length];
}

export const TAG_COLORS = ["#64748b", "#3b82f6", "#10b981", "#f97316", "#a855f7", "#ef4444", "#eab308", "#ec4899"];

export const AUDIO_EXTENSIONS = ["mp3", "wav", "m4a", "aac", "flac", "ogg", "opus", "webm", "mp4", "mov", "mkv", "wma", "aiff", "aif", "amr"];

export function isAudioPath(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return AUDIO_EXTENSIONS.includes(ext);
}

/** USD with enough precision for sub-cent amounts. */
export function formatUsd(v: number): string {
  if (!v) return "$0.00";
  if (v < 0.01) return `$${v.toFixed(4)}`;
  return `$${v.toFixed(2)}`;
}

export function formatMinutes(seconds: number): string {
  const m = seconds / 60;
  return m < 10 ? `${m.toFixed(1)} min` : `${Math.round(m).toLocaleString()} min`;
}

export function formatTokens(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

export function formatDay(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, { dateStyle: "medium" });
}

const languageNames = new Intl.DisplayNames(undefined, { type: "language" });

/** "en", "en-US", "fil" → display name; full names like "english" are capitalized. */
export function formatLanguage(lang: string | null): string {
  if (!lang) return "Auto";
  const l = lang.trim();
  if (/^[a-z]{2,3}([-_][a-z0-9]{2,8})*$/i.test(l)) {
    try {
      return languageNames.of(l.replace("_", "-")) ?? l;
    } catch {
      return l;
    }
  }
  return l.charAt(0).toUpperCase() + l.slice(1);
}

/** "1:32:05", "32:05" or "95" → seconds; null if invalid. */
export function parseClock(text: string): number | null {
  const parts = text.trim().split(":");
  if (!parts.length || parts.length > 3 || parts.some((p) => !/^\d+(\.\d+)?$/.test(p))) return null;
  return parts.reduce((acc, p) => acc * 60 + Number(p), 0);
}
