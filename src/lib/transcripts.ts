import type { TranscriptSummary } from "@/lib/api";

/** Notes that only say a blocked part was redone with another model: informational, not a problem. */
const INFO_NOTE = /transcribed with .+ instead$/;

/** Note lines describing parts that are still incomplete. */
export function problemNotes(t: TranscriptSummary): string[] {
  return (t.notes ?? "").split("\n").filter((n) => n.trim() && !INFO_NOTE.test(n.trim()));
}

/** True when the transcript failed, or finished with parts that may be incomplete. */
export function needsFix(t: TranscriptSummary): boolean {
  if (t.status === "error") return true;
  return t.status === "done" && (!!t.error || problemNotes(t).length > 0);
}

/** Short reason for the "Needs fix" badge. */
export function fixReason(t: TranscriptSummary): string {
  if (t.status === "error") return t.error ?? "Transcription failed";
  const notes = problemNotes(t);
  if (notes.length) return notes.length === 1 ? notes[0] : `${notes.length} parts may be incomplete`;
  if (t.error?.startsWith("Fixing timestamps failed")) return t.error;
  return t.error ? `Last re-transcription failed: ${t.error}` : "";
}
