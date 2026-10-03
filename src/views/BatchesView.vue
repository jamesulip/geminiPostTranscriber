<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useRouter } from "vue-router";
import { AlertTriangle, ExternalLink, Loader2, Moon, RefreshCw } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import { api, type BatchView } from "@/lib/api";
import { formatDate, formatUsd } from "@/lib/format";
import { useJobs } from "@/stores/jobs";

const router = useRouter();
const jobs = useJobs();
const batches = ref<BatchView[]>([]);
const warning = ref<string | null>(null);
const loading = ref(true);
const checking = ref(false);
let timer: ReturnType<typeof setInterval> | undefined;

async function load() {
  try {
    const r = await api.listBatches();
    batches.value = r.batches;
    warning.value = r.warning;
  } catch (e) {
    warning.value = String(e);
  } finally {
    loading.value = false;
  }
}

/** Ask Google about waiting batches now (collects finished ones), then refresh the list. */
async function checkNow() {
  checking.value = true;
  try {
    await api.checkBatches();
    await load();
  } catch (e) {
    toast.error("Couldn't check", { description: String(e) });
  } finally {
    checking.value = false;
  }
}

onMounted(() => {
  void load();
  timer = setInterval(load, 30_000);
});
onUnmounted(() => clearInterval(timer));

const waiting = (b: BatchView) => b.state === "PENDING" || b.state === "RUNNING";
const counts = computed(() => ({
  waiting: batches.value.filter(waiting).length,
  done: batches.value.filter((b) => b.state === "SUCCEEDED").length,
  problems: batches.value.filter((b) => (b.blocked ?? 0) > 0 || ["FAILED", "EXPIRED"].includes(b.state)).length,
  cost: batches.value.reduce((a, b) => a + (b.cost_usd ?? 0), 0),
}));

function took(b: BatchView) {
  if (!b.created) return "—";
  const end = b.ended ? new Date(b.ended).getTime() : Date.now();
  const min = (end - new Date(b.created).getTime()) / 60000;
  if (min < 1) return "<1 min";
  return min < 120 ? `${Math.round(min)} min` : `${(min / 60).toFixed(1)} h`;
}

const STATE: Record<string, { label: string; cls: string }> = {
  PENDING: { label: "Queued at Google", cls: "bg-muted text-foreground" },
  RUNNING: { label: "Processing", cls: "bg-blue-100 text-blue-800 dark:bg-blue-950 dark:text-blue-300" },
  SUCCEEDED: { label: "Done", cls: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300" },
  FAILED: { label: "Failed", cls: "bg-red-100 text-red-800 dark:bg-red-950 dark:text-red-300" },
  CANCELLED: { label: "Cancelled", cls: "bg-muted text-muted-foreground" },
  EXPIRED: { label: "Expired", cls: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300" },
};
const stateOf = (b: BatchView) => STATE[b.state] ?? { label: b.state || "Unknown", cls: "bg-muted" };
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex items-center gap-3 border-b px-6 py-4" data-tauri-drag-region>
      <div>
        <h1 class="text-lg font-semibold">Overnight batches</h1>
        <p class="text-muted-foreground text-xs">
          Live from Google's Batch API. Refreshes every 30 s while this page is open.
        </p>
      </div>
      <Button variant="outline" size="sm" class="ml-auto" :disabled="checking" @click="checkNow">
        <RefreshCw :class="checking && 'animate-spin'" /> Check now
      </Button>
    </header>

    <div class="min-h-0 flex-1 overflow-y-auto p-6">
      <div class="mx-auto grid max-w-6xl gap-4">
        <div class="grid grid-cols-2 gap-4 lg:grid-cols-4">
          <div class="bg-card rounded-xl border p-4">
            <p class="text-muted-foreground text-xs">Waiting at Google</p>
            <p class="mt-1 text-2xl font-semibold tabular-nums">{{ counts.waiting }}</p>
          </div>
          <div class="bg-card rounded-xl border p-4">
            <p class="text-muted-foreground text-xs">Done</p>
            <p class="mt-1 text-2xl font-semibold tabular-nums">{{ counts.done }}</p>
          </div>
          <div class="bg-card rounded-xl border p-4">
            <p class="text-muted-foreground text-xs">With problems</p>
            <p class="mt-1 text-2xl font-semibold tabular-nums">{{ counts.problems }}</p>
          </div>
          <div class="bg-card rounded-xl border p-4">
            <p class="text-muted-foreground text-xs">Recorded cost</p>
            <p class="mt-1 text-2xl font-semibold tabular-nums">{{ formatUsd(counts.cost) }}</p>
          </div>
        </div>

        <p v-if="warning" class="flex items-center gap-2 rounded-lg border border-amber-200 bg-amber-50 px-4 py-2 text-sm dark:border-amber-900 dark:bg-amber-950/40">
          <AlertTriangle class="size-4 shrink-0 text-amber-600" /> {{ warning }}
        </p>

        <div v-if="loading" class="text-muted-foreground flex items-center gap-2 py-10 text-sm">
          <Loader2 class="size-4 animate-spin" /> Loading batches from Google…
        </div>
        <div v-else-if="!batches.length" class="text-muted-foreground rounded-xl border border-dashed p-10 text-center text-sm">
          <Moon class="mx-auto mb-2 size-6" /> No overnight batches yet. Choose <b>When → Overnight (batch)</b> when adding a recording.
        </div>
        <div v-else class="overflow-hidden rounded-xl border">
          <table class="w-full table-fixed text-sm">
            <thead class="bg-muted/40 text-muted-foreground text-left text-xs">
              <tr>
                <th class="w-[28%] px-3 py-2 font-medium">Meeting</th>
                <th class="px-3 py-2 font-medium">Status</th>
                <th class="px-3 py-2 font-medium">Sent</th>
                <th class="px-3 py-2 text-right font-medium">Took</th>
                <th class="px-3 py-2 text-right font-medium">Parts</th>
                <th class="w-[18%] px-3 py-2 font-medium">Problems</th>
                <th class="px-3 py-2 text-right font-medium">Cost</th>
                <th class="w-24 px-3 py-2" />
              </tr>
            </thead>
            <tbody>
              <tr v-for="b in batches" :key="b.name" class="hover:bg-accent/30 border-t align-top">
                <td class="px-3 py-2.5">
                  <button
                    v-if="b.transcript_id"
                    class="hover:text-primary block w-full truncate text-left font-medium hover:underline"
                    :title="b.title"
                    @click="router.push({ name: 'transcript', params: { id: b.transcript_id } })"
                  >{{ b.title }}</button>
                  <span v-else class="block truncate font-medium" :title="b.title">{{ b.title || "(untitled)" }}</span>
                  <span class="text-muted-foreground block truncate font-mono text-[10px]" :title="b.name">{{ b.name }} · {{ b.model }}</span>
                </td>
                <td class="px-3 py-2.5">
                  <span class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium" :class="stateOf(b).cls">
                    <Loader2 v-if="waiting(b)" class="size-3 animate-spin" />{{ stateOf(b).label }}
                  </span>
                </td>
                <td class="text-muted-foreground px-3 py-2.5 text-xs">{{ b.created ? formatDate(b.created) : "—" }}</td>
                <td class="px-3 py-2.5 text-right tabular-nums">{{ took(b) }}</td>
                <td class="px-3 py-2.5 text-right tabular-nums">{{ b.parts ?? "—" }}</td>
                <td class="px-3 py-2.5 text-xs">
                  <span v-if="(b.blocked ?? 0) > 0" class="text-amber-700 dark:text-amber-400">
                    {{ b.blocked }} blocked/empty{{ b.retried ? " · re-done with Gemini 3.5" : " · use Fix problems" }}
                  </span>
                  <span v-else-if="b.outcome?.startsWith('error')" class="text-destructive">{{ b.outcome.slice(7) }}</span>
                  <span v-else-if="b.blocked === 0" class="text-muted-foreground">none</span>
                  <span v-else class="text-muted-foreground" title="Sent before the app recorded per-part results">—</span>
                </td>
                <td class="px-3 py-2.5 text-right tabular-nums">{{ b.cost_usd != null ? formatUsd(b.cost_usd) : "—" }}</td>
                <td class="px-3 py-2.5 text-right">
                  <Button
                    v-if="waiting(b) && b.transcript_id"
                    variant="ghost"
                    size="sm"
                    :disabled="jobs.cancelling.has(b.transcript_id)"
                    @click="jobs.cancel(b.transcript_id).then(load)"
                  >Cancel</Button>
                  <Button
                    v-else-if="b.transcript_id"
                    variant="ghost"
                    size="sm"
                    @click="router.push({ name: 'transcript', params: { id: b.transcript_id } })"
                  ><ExternalLink /> Open</Button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <p class="text-muted-foreground text-xs">
          Google reports blocked parts as “succeeded”; the <b>Problems</b> column shows what the app actually received.
          Costs are the app's estimates at batch prices — Google's billing in AI Studio is the exact figure.
        </p>
      </div>
    </div>
  </div>
</template>
