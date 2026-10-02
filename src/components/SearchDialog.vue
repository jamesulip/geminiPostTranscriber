<script setup lang="ts">
import { ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useDebounceFn, useEventListener } from "@vueuse/core";
import { FileText, Search } from "@lucide/vue";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { api, type SearchHit } from "@/lib/api";
import { formatTime } from "@/lib/format";
import { snippetHtml } from "@/lib/highlight";
import { useUi } from "@/stores/ui";

const ui = useUi();
const router = useRouter();
const query = ref("");
const hits = ref<SearchHit[]>([]);
const active = ref(0);
const searched = ref(false);

useEventListener(window, "keydown", (e: KeyboardEvent) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    ui.searchOpen = !ui.searchOpen;
  }
});

const run = useDebounceFn(async (q: string) => {
  hits.value = q.trim() ? await api.search(q) : [];
  active.value = 0;
  searched.value = !!q.trim();
}, 150);
watch(query, (q) => run(q));
watch(
  () => ui.searchOpen,
  (o) => {
    if (o) {
      query.value = "";
      hits.value = [];
      searched.value = false;
    }
  },
);

function open(h: SearchHit) {
  ui.searchOpen = false;
  router.push({
    name: "transcript",
    params: { id: h.transcript_id },
    query: h.segment_id >= 0 ? { t: String(h.start), seg: String(h.segment_id) } : {},
  });
}

function onKey(e: KeyboardEvent) {
  if (e.key === "ArrowDown") active.value = Math.min(active.value + 1, hits.value.length - 1);
  else if (e.key === "ArrowUp") active.value = Math.max(active.value - 1, 0);
  else if (e.key === "Enter" && hits.value[active.value]) open(hits.value[active.value]);
  else return;
  e.preventDefault();
  document.getElementById(`hit-${active.value}`)?.scrollIntoView({ block: "nearest" });
}
</script>

<template>
  <Dialog v-model:open="ui.searchOpen">
    <DialogContent class="top-[15%] translate-y-0 gap-0 overflow-hidden p-0 sm:max-w-2xl" :show-close-button="false">
      <DialogHeader class="sr-only">
        <DialogTitle>Search transcripts</DialogTitle>
        <DialogDescription>Full-text search across all transcripts</DialogDescription>
      </DialogHeader>
      <div class="flex items-center gap-2 border-b px-4">
        <Search class="text-muted-foreground size-4" />
        <input
          v-model="query"
          autofocus
          placeholder="Search all transcripts…"
          class="h-12 flex-1 bg-transparent text-sm outline-none"
          @keydown="onKey"
        />
      </div>
      <div class="max-h-[60vh] overflow-y-auto p-2">
        <button
          v-for="(h, i) in hits"
          :id="`hit-${i}`"
          :key="`${h.transcript_id}-${h.segment_id}`"
          class="flex w-full flex-col gap-0.5 rounded-lg px-3 py-2 text-left"
          :class="i === active ? 'bg-accent' : 'hover:bg-accent/50'"
          @mouseenter="active = i"
          @click="open(h)"
        >
          <div class="text-muted-foreground flex items-center gap-2 text-xs">
            <FileText class="size-3.5" />
            <span class="text-foreground truncate font-medium">{{ h.transcript_title }}</span>
            <template v-if="h.segment_id >= 0">
              <span class="tabular-nums">{{ formatTime(h.start) }}</span>
              <span v-if="h.speaker">· {{ h.speaker }}</span>
            </template>
          </div>
          <div v-if="h.segment_id >= 0" class="search-snippet text-sm" v-html="snippetHtml(h.snippet)" />
        </button>
        <p v-if="searched && !hits.length" class="text-muted-foreground p-6 text-center text-sm">No matches</p>
        <p v-if="!searched" class="text-muted-foreground p-6 text-center text-sm">Type to search words spoken in any transcript</p>
      </div>
    </DialogContent>
  </Dialog>
</template>
