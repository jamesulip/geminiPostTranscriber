<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { RefreshCw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { api, type UsageKind, type UsageSummary, type UsageTotals } from "@/lib/api";
import { formatDate, formatMinutes, formatTokens, formatUsd } from "@/lib/format";

const router = useRouter();
const data = ref<UsageSummary | null>(null);

async function load() {
  data.value = await api.usageSummary();
}
onMounted(load);

const savedPct = (t: UsageTotals) =>
  t.original_seconds ? Math.round((1 - t.sent_seconds / t.original_seconds) * 100) : 0;

const months = computed(() => {
  const groups = new Map<string, UsageSummary["by_month"]>();
  for (const g of data.value?.by_month ?? []) {
    if (!groups.has(g.month)) groups.set(g.month, []);
    groups.get(g.month)!.push(g);
  }
  return [...groups.entries()].map(([month, rows]) => ({
    month,
    label: new Date(`${month}-01T00:00:00`).toLocaleDateString(undefined, { year: "numeric", month: "long" }),
    rows,
    cost: rows.reduce((a, r) => a + r.cost_usd, 0),
  }));
});

const KIND_LABEL: Record<UsageKind, string> = { transcribe: "Transcription", ai: "AI assistant", embed: "Search index" };
const kindOf = (k: string) => KIND_LABEL[k as UsageKind] ?? k;

const tiles = computed(() => {
  const d = data.value;
  if (!d) return [];
  const aiSpend = d.by_kind.filter((k) => k.kind !== "transcribe").reduce((a, k) => a + k.cost_usd, 0);
  const aiReqs = d.by_kind.filter((k) => k.kind !== "transcribe").reduce((a, k) => a + k.requests, 0);
  return [
    { label: "AI assistant (all time)", value: formatUsd(aiSpend), sub: `${aiReqs} request(s) · summaries, questions, search index` },
    { label: "Spent this month", value: formatUsd(d.this_month.cost_usd), sub: `${d.this_month.jobs} job(s)` },
    { label: "Spent all time", value: formatUsd(d.all_time.cost_usd), sub: `${d.all_time.requests} request(s)` },
    {
      label: "Audio sent",
      value: formatMinutes(d.all_time.sent_seconds),
      sub: `of ${formatMinutes(d.all_time.original_seconds)} recorded`,
    },
    {
      label: "Saved by condensing",
      value: formatUsd(d.all_time.saved_usd),
      sub: `${savedPct(d.all_time)}% less audio sent`,
    },
  ];
});
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex items-center border-b px-6 py-4" data-tauri-drag-region>
      <div>
        <h1 class="text-lg font-semibold">Usage</h1>
        <p class="text-muted-foreground text-xs">Transcription: audio minutes sent × price per minute, or reported tokens × token prices. AI assistant: reported tokens × text prices (Settings).</p>
      </div>
      <Button variant="outline" size="sm" class="ml-auto" @click="load"><RefreshCw /> Refresh</Button>
    </header>

    <div v-if="data" class="min-h-0 flex-1 overflow-y-auto p-6">
      <div class="mx-auto grid max-w-5xl gap-6">
        <div class="grid grid-cols-2 gap-4 lg:grid-cols-5">
          <div v-for="t in tiles" :key="t.label" class="bg-card rounded-xl border p-4">
            <p class="text-muted-foreground text-xs">{{ t.label }}</p>
            <p class="mt-1 text-2xl font-semibold tabular-nums">{{ t.value }}</p>
            <p class="text-muted-foreground mt-0.5 text-xs">{{ t.sub }}</p>
          </div>
        </div>

        <p v-if="!data.recent.length" class="text-muted-foreground rounded-xl border border-dashed p-10 text-center text-sm">
          No usage yet. Costs appear here after your first transcription.
        </p>

        <Card v-for="m in months" :key="m.month">
          <CardHeader>
            <CardTitle class="flex items-baseline justify-between">
              {{ m.label }} <span class="text-base tabular-nums">{{ formatUsd(m.cost) }}</span>
            </CardTitle>
          </CardHeader>
          <CardContent>
            <table class="w-full table-fixed text-sm">
              <thead class="text-muted-foreground text-left text-xs">
                <tr>
                  <th class="w-[34%] pb-2 font-medium">Provider · model</th>
                  <th class="pb-2 text-right font-medium">Jobs</th>
                  <th class="pb-2 text-right font-medium">Recorded</th>
                  <th class="pb-2 text-right font-medium">Sent</th>
                  <th class="pb-2 text-right font-medium">Tokens in / out</th>
                  <th class="pb-2 text-right font-medium">Saved</th>
                  <th class="pb-2 text-right font-medium">Cost</th>
                </tr>
              </thead>
              <tbody class="tabular-nums">
                <tr v-for="r in m.rows" :key="r.kind + r.provider_name + r.model" class="border-t">
                  <td class="truncate py-2">
                    <span v-if="r.kind !== 'transcribe'" class="bg-primary/10 text-primary mr-1.5 rounded px-1.5 py-0.5 text-[10px] font-medium">{{ kindOf(r.kind) }}</span>
                    {{ r.provider_name }} <span class="text-muted-foreground">· {{ r.model }}</span>
                  </td>
                  <td class="py-2 text-right">{{ r.jobs }}</td>
                  <td class="py-2 text-right">{{ formatMinutes(r.original_seconds) }}</td>
                  <td class="py-2 text-right">{{ formatMinutes(r.sent_seconds) }}</td>
                  <td class="text-muted-foreground py-2 text-right">
                    {{ r.input_tokens || r.output_tokens ? `${formatTokens(r.input_tokens)} / ${formatTokens(r.output_tokens)}` : "—" }}
                  </td>
                  <td class="py-2 text-right text-emerald-600">{{ formatUsd(r.saved_usd) }}</td>
                  <td class="py-2 text-right font-medium">{{ formatUsd(r.cost_usd) }}</td>
                </tr>
              </tbody>
            </table>
          </CardContent>
        </Card>

        <Card v-if="data.recent.length">
          <CardHeader>
            <CardTitle>Recent jobs</CardTitle>
            <CardDescription>Failed or cancelled jobs still count the chunks that were sent. AI rows show tokens in / out.</CardDescription>
          </CardHeader>
          <CardContent>
            <table class="w-full table-fixed text-sm">
              <thead class="text-muted-foreground text-left text-xs">
                <tr>
                  <th class="w-[30%] pb-2 font-medium">Transcript</th>
                  <th class="pb-2 font-medium">When</th>
                  <th class="pb-2 font-medium">Provider</th>
                  <th class="pb-2 text-right font-medium">Sent</th>
                  <th class="pb-2 text-right font-medium">Cost</th>
                </tr>
              </thead>
              <tbody class="tabular-nums">
                <tr v-for="r in data.recent" :key="r.id" class="border-t">
                  <td class="truncate py-2">
                    <button
                      v-if="r.transcript_id"
                      class="hover:text-primary truncate text-left hover:underline"
                      @click="router.push({ name: 'transcript', params: { id: r.transcript_id } })"
                    >{{ r.title }}</button>
                    <span v-else class="text-muted-foreground truncate">{{ r.title }} (deleted)</span>
                    <span v-if="r.status === 'error'" class="text-destructive ml-1.5 text-xs">failed</span>
                    <span v-if="r.kind !== 'transcribe'" class="bg-primary/10 text-primary ml-1.5 rounded px-1.5 py-0.5 text-[10px] font-medium">{{ kindOf(r.kind) }}</span>
                  </td>
                  <td class="text-muted-foreground py-2">{{ formatDate(r.created_at) }}</td>
                  <td class="truncate py-2">{{ r.provider_name }} <span class="text-muted-foreground">· {{ r.model }}</span></td>
                  <td class="py-2 text-right">{{ r.kind === 'transcribe' ? formatMinutes(r.sent_seconds) : `${formatTokens(r.input_tokens)} / ${formatTokens(r.output_tokens)}` }}</td>
                  <td class="py-2 text-right font-medium">{{ formatUsd(r.cost_usd) }}</td>
                </tr>
              </tbody>
            </table>
          </CardContent>
        </Card>
      </div>
    </div>
  </div>
</template>
