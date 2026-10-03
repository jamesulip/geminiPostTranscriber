import { defineStore } from "pinia";
import { reactive } from "vue";
import { listen } from "@tauri-apps/api/event";
import { toast } from "vue-sonner";
import { api, type Progress } from "@/lib/api";
import { useLibrary } from "./library";

export const useJobs = defineStore("jobs", () => {
  const progress = reactive<Record<string, Progress>>({});
  /** Jobs the user asked to cancel (shown as "Cancelling…" until they stop). */
  const cancelling = reactive(new Set<string>());

  async function cancel(id: string) {
    cancelling.add(id);
    await api.cancelJob(id);
  }
  let started = false;

  async function start() {
    if (started) return;
    started = true;
    const library = useLibrary();
    await listen<Progress>("job-progress", (e) => {
      const p = e.payload;
      if (p.done) {
        delete progress[p.id];
        const wasCancelled = cancelling.delete(p.id) && p.error === "Cancelled";
        const title = library.transcripts.find((t) => t.id === p.id)?.title ?? "Transcript";
        if (p.stage === "queued") toast(`${title}: sent as an overnight batch`, { description: "Results usually arrive within 24 hours. Keep the app open now and then to collect them." });
        else if (wasCancelled) toast(`${title} cancelled`, { description: "Finished chunks are kept; Retry continues from there." });
        else if (p.error) toast.error(`${title} failed`, { description: p.error });
        else if (p.stage === "diarize") toast.success(`${title}: ${p.message || "speakers found"}`, { description: "The previous speaker labels were kept in the history." });
        else toast.success(`${title} transcribed`);
      } else {
        progress[p.id] = p;
      }
      // Refresh on stage changes and completion so durations/status stay current.
      void library.refresh();
    });
    // A reloaded window doesn't receive past events; ask for the current status of running jobs.
    for (const p of await api.jobProgress()) progress[p.id] = p;
  }

  return { progress, cancelling, cancel, start };
});
