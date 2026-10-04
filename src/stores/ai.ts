import { defineStore } from "pinia";
import { reactive, ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import { toast } from "vue-sonner";
import { api, type AiAnswerEvent, type AiEvent, type AskParams } from "@/lib/api";

type AnswerListener = (e: AiAnswerEvent) => void;

/** Live state of the AI assistant: running jobs, streamed answer text, final answers. */
export const useAi = defineStore("ai", () => {
  /** Background work by key (`digest:<id>`, `ask:<thread>`, `embed:library`). */
  const running = reactive<Record<string, { message: string; progress: number }>>({});
  /** Text streamed so far for a thread whose answer is in flight. */
  const streams = reactive<Record<string, string>>({});
  /** Last finished digest per transcript id, bumped so panels can refetch. */
  const digestVersion = reactive<Record<string, number>>({});
  const listeners = new Set<AnswerListener>();
  const started = ref(false);

  function onAnswer(cb: AnswerListener) {
    listeners.add(cb);
    return () => listeners.delete(cb);
  }

  function isRunning(key: string) {
    return key in running;
  }

  async function start() {
    if (started.value) return;
    started.value = true;
    await listen<AiEvent>("ai-event", (e) => {
      const p = e.payload;
      if (p.type === "token") {
        streams[p.thread_id] = (streams[p.thread_id] ?? "") + p.delta;
        return;
      }
      if (p.done) {
        delete running[p.key];
        if (p.key.startsWith("digest:") || p.key.startsWith("translate:")) {
          const id = p.key.slice(p.key.indexOf(":") + 1);
          digestVersion[id] = (digestVersion[id] ?? 0) + 1;
          if (p.error && p.error !== "Cancelled") toast.error(p.key.startsWith("digest:") ? "Summary failed" : "Translation failed", { description: p.error });
        } else if (p.key === "embed:library") {
          if (p.error) toast.error("Indexing failed", { description: p.error });
          else toast.success(p.message || "Library indexed");
        }
      } else {
        running[p.key] = { message: p.message, progress: p.progress };
      }
    });
    await listen<AiAnswerEvent>("ai-answer", (e) => {
      delete streams[e.payload.thread_id];
      for (const cb of listeners) cb(e.payload);
    });
    for (const key of await api.aiRunning()) running[key] = { message: "Working…", progress: 0 };
  }

  async function digest(id: string, force = false) {
    running[`digest:${id}`] = { message: "Starting", progress: 0 };
    try {
      await api.aiDigest(id, force);
    } catch (e) {
      delete running[`digest:${id}`];
      toast.error("Couldn't start the summary", { description: String(e) });
    }
  }

  /** Returns the thread id; the answer arrives through `onAnswer`. */
  async function ask(params: AskParams): Promise<string> {
    const threadId = await api.aiAsk(params);
    streams[threadId] = "";
    running[`ask:${threadId}`] = { message: "Thinking", progress: 0 };
    return threadId;
  }

  async function cancel(key: string) {
    await api.aiCancel(key);
  }

  return { running, streams, digestVersion, start, onAnswer, isRunning, digest, ask, cancel };
});
