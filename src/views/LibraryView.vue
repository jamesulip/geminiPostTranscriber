<script setup lang="ts">
import { computed, ref } from "vue";
import { Search, Upload } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import TranscriptCard from "@/components/TranscriptCard.vue";
import { useLibrary } from "@/stores/library";
import { useUi } from "@/stores/ui";
import { pickAudioFiles } from "@/lib/files";

const library = useLibrary();
const ui = useUi();
const filter = ref("");

const visible = computed(() => {
  const q = filter.value.trim().toLowerCase();
  if (!q) return library.transcripts;
  return library.transcripts.filter(
    (t) => t.title.toLowerCase().includes(q) || t.source_name.toLowerCase().includes(q) || t.tags.some((g) => g.name.includes(q)),
  );
});

async function upload() {
  const files = await pickAudioFiles();
  if (files.length) ui.pendingFiles = files;
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex items-center gap-3 border-b px-6 py-4" data-tauri-drag-region>
      <div>
        <h1 class="text-lg font-semibold">{{ library.selectionLabel }}</h1>
        <p class="text-muted-foreground text-xs">{{ library.transcripts.length }} transcript(s)</p>
      </div>
      <div class="relative ml-auto w-64">
        <Search class="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2" />
        <Input v-model="filter" placeholder="Filter by title or tag" class="pl-8" />
      </div>
      <Button @click="upload"><Upload /> Upload</Button>
    </header>

    <div class="min-h-0 flex-1 overflow-y-auto p-6">
      <button
        v-if="!library.transcripts.length && !library.loading"
        class="hover:border-primary/60 hover:bg-accent/30 flex h-72 w-full flex-col items-center justify-center gap-3 rounded-2xl border-2 border-dashed transition-colors"
        @click="upload"
      >
        <Upload class="text-muted-foreground size-10" />
        <p class="font-medium">Drop an audio file here, or click to upload</p>
        <p class="text-muted-foreground text-sm">MP3, WAV, M4A, FLAC, OGG, video files…</p>
      </button>
      <div v-else class="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4">
        <TranscriptCard v-for="t in visible" :key="t.id" :t="t" />
      </div>
      <p v-if="library.transcripts.length && !visible.length" class="text-muted-foreground p-6 text-center text-sm">
        Nothing matches “{{ filter }}”. Use ⌘K to search inside transcripts.
      </p>
    </div>
  </div>
</template>
