<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useDebounceFn, useEventListener } from "@vueuse/core";
import { FileText, Search, Sparkles } from "@lucide/vue";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { api, type AiHit, type SearchHit } from "@/lib/api";
import { formatTime } from "@/lib/format";
import { snippetHtml } from "@/lib/highlight";
import { useUi } from "@/stores/ui";

const ui = useUi();
const router = useRouter();
const query = ref("");
const hits = ref<SearchHit[]>([]);
const active = ref(0);
const searched = ref(false);
const meaning = ref<AiHit[]>([]);
const meaningNote = ref<string | null>(null);
/** Long enough to be a question rather than a word lookup. */
const askable = computed(() => query.value.trim().length >= 12);

// Meaning search is an API call (one embedding): debounce it more and only for real phrases.
const runMeaning = useDebounceFn(async (q: string) => {
  if (q.trim().length < 3) {
    meaning.value = [];
    return;
  }
  try {
    const r = await api.aiSearch(q, { kind: "library" }, 8);
    if (q === query.value) {
      meaning.value = r.hits;
      meaningNote.value = r.note;
    }
  } catch (e) {
    meaningNote.value = String(e);
  }
}, 500);

function ask() {
  const q = query.value.trim();
  ui.searchOpen = false;
  router.push({ name: "ask", query: { q } });
}

function openHit(h: AiHit) {
  ui.searchOpen = false;
  router.push({ name: "transcript", params: { id: h.transcript_id }, query: { t: String(h.at) } });
}

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
watch(query, (q) => {
  run(q);
  runMeaning(q);
});
watch(
  () => ui.searchOpen,
  (o) => {
    if (o) {
      query.value = "";
      hits.value = [];
      meaning.value = [];
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
  else if (e.key === "Enter" && askable.value) ask();
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
          v-if="askable"
          class="hover:bg-accent/50 mb-1 flex w-full items-center gap-2 rounded-lg border border-dashed px-3 py-2 text-left text-sm"
          @click="ask"
        >
          <Sparkles class="text-primary size-4 shrink-0" />
          <span class="min-w-0 truncate">Ask the assistant: <b>{{ query.trim() }}</b></span>
          <span class="text-muted-foreground ml-auto shrink-0 text-xs">answers with citations</span>
        </button>
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
        <p v-if="searched && !hits.length" class="text-muted-foreground p-6 text-center text-sm">No word matches</p>
        <template v-if="meaning.length">
          <p class="text-muted-foreground mt-2 mb-1 px-3 text-[11px] font-medium uppercase">By meaning</p>
          <button
            v-for="h in meaning"
            :key="h.chunk_id"
            class="hover:bg-accent/50 flex w-full flex-col gap-0.5 rounded-lg px-3 py-2 text-left"
            @click="openHit(h)"
          >
            <div class="text-muted-foreground flex items-center gap-2 text-xs">
              <Sparkles class="size-3.5" />
              <span class="text-foreground truncate font-medium">{{ h.title }}</span>
              <span class="tabular-nums">{{ formatTime(h.at) }}</span>
              <span class="bg-muted rounded px-1 text-[10px]">{{ h.via === "both" ? "words + meaning" : h.via }}</span>
            </div>
            <div class="text-muted-foreground line-clamp-2 text-sm">{{ h.text.replace(/^\[[^\]]+\]\s*/gm, "").slice(0, 240) }}</div>
          </button>
        </template>
        <p v-else-if="searched && meaningNote" class="text-muted-foreground px-3 pb-2 text-[11px]">{{ meaningNote }}</p>
        <p v-if="!searched" class="text-muted-foreground p-6 text-center text-sm">Type to search words spoken in any transcript</p>
      </div>
    </DialogContent>
  </Dialog>
</template>
