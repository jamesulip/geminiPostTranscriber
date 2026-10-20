import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { toast } from "vue-sonner";
import { useJobs } from "./jobs";

type Status = "idle" | "checking" | "available" | "downloading" | "ready" | "error";

/** Checks GitHub's latest release (latest.json) a few seconds after launch and every 6 h. */
export const useUpdates = defineStore("updates", () => {
  const version = ref("");
  const status = ref<Status>("idle");
  const update = ref<Update | null>(null);
  const error = ref("");
  const received = ref(0);
  const total = ref(0);
  const percent = computed(() => (total.value ? Math.round((received.value / total.value) * 100) : 0));
  let started = false;

  async function checkNow(quiet = false) {
    if (status.value === "checking" || status.value === "downloading" || status.value === "ready") return;
    status.value = "checking";
    error.value = "";
    try {
      const found = await check();
      update.value = found;
      status.value = found ? "available" : "idle";
      if (found && quiet) {
        toast(`Transcriber ${found.version} is available`, {
          description: "Download and install it now; the app restarts when you're ready.",
          duration: Infinity,
          action: { label: "Update", onClick: () => void install() },
        });
      } else if (!quiet) {
        if (found) toast(`Transcriber ${found.version} is available`);
        else toast.success("Transcriber is up to date", { description: `Version ${version.value}` });
      }
    } catch (e) {
      status.value = "error";
      error.value = String(e);
      if (!quiet) toast.error("Couldn't check for updates", { description: error.value });
    }
  }

  async function install() {
    const u = update.value;
    if (!u || status.value === "downloading" || status.value === "ready") return;
    status.value = "downloading";
    received.value = 0;
    total.value = 0;
    const id = toast.loading(`Downloading Transcriber ${u.version}…`);
    try {
      // On Windows the installer closes the app itself once this resolves.
      await u.downloadAndInstall((e) => {
        if (e.event === "Started") total.value = e.data.contentLength ?? 0;
        else if (e.event === "Progress") {
          received.value += e.data.chunkLength;
          if (total.value) toast.loading(`Downloading Transcriber ${u.version}… ${percent.value}%`, { id });
        }
      });
      status.value = "ready";
      const running = Object.keys(useJobs().progress).length;
      toast.success(`Transcriber ${u.version} installed`, {
        id,
        description: running
          ? `${running} running job${running === 1 ? "" : "s"} will stop on restart; Retry continues from finished chunks.`
          : "Restart to start using it.",
        duration: Infinity,
        action: { label: "Restart now", onClick: () => void relaunch() },
      });
    } catch (e) {
      status.value = "error";
      error.value = String(e);
      toast.error("Update failed", { id, description: error.value });
    }
  }

  async function start() {
    if (started) return;
    started = true;
    version.value = await getVersion().catch(() => "");
    // Dev builds share the release version; don't nag while developing.
    if (import.meta.env.DEV) return;
    setTimeout(() => void checkNow(true), 5_000);
    setInterval(() => void checkNow(true), 6 * 60 * 60 * 1000);
  }

  return { version, status, update, error, percent, checkNow, install, relaunch, start };
});
