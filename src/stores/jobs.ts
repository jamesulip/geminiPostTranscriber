import { defineStore } from "pinia";
import { reactive } from "vue";
import { listen } from "@tauri-apps/api/event";
import { toast } from "vue-sonner";
import type { Progress } from "@/lib/api";
import { useLibrary } from "./library";

export const useJobs = defineStore("jobs", () => {
  const progress = reactive<Record<string, Progress>>({});
  let started = false;

  async function start() {
    if (started) return;
    started = true;
    const library = useLibrary();
    await listen<Progress>("job-progress", (e) => {
      const p = e.payload;
      if (p.done) {
        delete progress[p.id];
        const title = library.transcripts.find((t) => t.id === p.id)?.title ?? "Transcript";
        if (p.error) toast.error(`${title} failed`, { description: p.error });
        else toast.success(`${title} transcribed`);
      } else {
        progress[p.id] = p;
      }
      // Refresh on stage changes and completion so durations/status stay current.
      void library.refresh();
    });
  }

  return { progress, start };
});
