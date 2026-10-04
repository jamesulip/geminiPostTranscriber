<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Flag, LayoutGrid, List, Search, Trash2, Upload, Wrench, X } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import TranscriptCard from "@/components/TranscriptCard.vue";
import TranscriptRow from "@/components/TranscriptRow.vue";
import { useLibrary } from "@/stores/library";
import { useUi } from "@/stores/ui";
import { pickAudioFiles } from "@/lib/files";
import { needsFix } from "@/lib/transcripts";

const library = useLibrary();
const ui = useUi();
const filter = ref("");
type Chip = "all" | "flagged" | "fix";
const chip = ref<Chip>("all");
const confirmEmpty = ref(false);
const inTrash = computed(() => library.selection.kind === "trash");
watch(inTrash, () => (chip.value = "all"));

const counts = computed(() => ({
  all: library.transcripts.length,
  flagged: library.transcripts.filter((t) => t.flagged).length,
  fix: library.transcripts.filter(needsFix).length,
}));

const visible = computed(() => {
  const q = filter.value.trim().toLowerCase();
  return library.transcripts.filter((t) => {
    if (chip.value === "flagged" && !t.flagged) return false;
    if (chip.value === "fix" && !needsFix(t)) return false;
    if (!q) return true;
    return (
      t.title.toLowerCase().includes(q) ||
      t.source_name.toLowerCase().includes(q) ||
      t.tags.some((g) => g.name.toLowerCase().includes(q))
    );
  });
});
const filtering = computed(() => !!filter.value.trim() || chip.value !== "all");

async function upload() {
  const files = await pickAudioFiles();
  if (files.length) ui.pendingFiles = files;
}

async function emptyTrash() {
  confirmEmpty.value = false;
  try {
    await library.emptyTrash();
    toast.success("Trash emptied");
  } catch (e) {
    toast.error("Couldn't empty the trash", { description: String(e) });
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex items-center gap-3 border-b px-6 py-4" data-tauri-drag-region>
      <div>
        <h1 class="flex items-center gap-2 text-lg font-semibold">
          <Trash2 v-if="inTrash" class="text-muted-foreground size-5" /> {{ library.selectionLabel }}
        </h1>
        <p class="text-muted-foreground text-xs">
          <template v-if="filtering">{{ visible.length }} of {{ library.transcripts.length }} transcript(s)</template>
          <template v-else>{{ library.transcripts.length }} transcript(s)</template>
        </p>
      </div>
      <div class="ml-auto flex items-center gap-2">
        <Button v-if="inTrash" variant="destructive" :disabled="!library.transcripts.length" @click="confirmEmpty = true">
          <Trash2 /> Empty trash
        </Button>
        <Button v-else @click="upload"><Upload /> Upload</Button>
      </div>
    </header>

    <div class="flex items-center gap-2 border-b px-6 py-2">
      <div class="relative min-w-0 flex-1 max-w-md">
        <Search class="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2" />
        <Input v-model="filter" placeholder="Search by title, file name or tag" class="pr-8 pl-8" />
        <button
          v-if="filter"
          class="text-muted-foreground hover:text-foreground absolute top-1/2 right-2 -translate-y-1/2"
          title="Clear"
          @click="filter = ''"
        >
          <X class="size-4" />
        </button>
      </div>

      <div v-if="!inTrash" class="flex items-center gap-1">
        <Button size="sm" :variant="chip === 'all' ? 'secondary' : 'ghost'" @click="chip = 'all'">All</Button>
        <Button size="sm" :variant="chip === 'flagged' ? 'secondary' : 'ghost'" :class="chip !== 'flagged' && 'text-muted-foreground'" @click="chip = 'flagged'">
          <Flag class="text-amber-500" fill="currentColor" /> Flagged
          <span v-if="counts.flagged" class="tabular-nums">{{ counts.flagged }}</span>
        </Button>
        <Button size="sm" :variant="chip === 'fix' ? 'secondary' : 'ghost'" :class="chip !== 'fix' && 'text-muted-foreground'" @click="chip = 'fix'">
          <Wrench /> Needs fix
          <span v-if="counts.fix" class="tabular-nums">{{ counts.fix }}</span>
        </Button>
      </div>

      <div class="ml-auto flex items-center gap-0.5 rounded-lg border p-0.5">
        <Button size="icon-sm" :variant="ui.libraryView === 'grid' ? 'secondary' : 'ghost'" title="Grid view" @click="ui.libraryView = 'grid'">
          <LayoutGrid />
        </Button>
        <Button size="icon-sm" :variant="ui.libraryView === 'list' ? 'secondary' : 'ghost'" title="List view" @click="ui.libraryView = 'list'">
          <List />
        </Button>
      </div>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto p-6">
      <div v-if="inTrash && !library.transcripts.length && !library.loading" class="text-muted-foreground flex h-72 flex-col items-center justify-center gap-3">
        <Trash2 class="size-10" />
        <p class="font-medium">Trash is empty</p>
        <p class="text-sm">Transcripts you move to the trash stay here until you delete them forever.</p>
      </div>
      <button
        v-else-if="!library.transcripts.length && !library.loading"
        class="hover:border-primary/60 hover:bg-accent/30 flex h-72 w-full flex-col items-center justify-center gap-3 rounded-2xl border-2 border-dashed transition-colors"
        @click="upload"
      >
        <Upload class="text-muted-foreground size-10" />
        <p class="font-medium">Drop an audio file here, or click to upload</p>
        <p class="text-muted-foreground text-sm">MP3, WAV, M4A, FLAC, OGG, video files…</p>
      </button>
      <div v-else-if="ui.libraryView === 'grid'" class="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4">
        <TranscriptCard v-for="t in visible" :key="t.id" :t="t" />
      </div>
      <div v-else-if="visible.length" class="bg-card overflow-hidden rounded-xl border">
        <TranscriptRow v-for="t in visible" :key="t.id" :t="t" />
      </div>
      <p v-if="library.transcripts.length && !visible.length" class="text-muted-foreground p-6 text-center text-sm">
        <template v-if="filter.trim()">Nothing matches “{{ filter }}”. Use ⌘K to search inside transcripts.</template>
        <template v-else-if="chip === 'flagged'">No flagged transcripts. Use the flag on a transcript to mark it for attention.</template>
        <template v-else>Nothing needs fixing.</template>
      </p>
    </div>

    <ConfirmDialog
      v-if="confirmEmpty"
      title="Empty the trash?"
      :description="`${library.transcripts.length} transcript(s) and their audio copies will be removed. This can't be undone.`"
      confirm-label="Empty trash"
      destructive
      @close="confirmEmpty = false"
      @confirm="emptyTrash"
    />
  </div>
</template>
