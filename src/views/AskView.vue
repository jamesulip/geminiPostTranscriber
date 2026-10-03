<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { FileText, Loader2, Sparkles } from "@lucide/vue";
import { save } from "@tauri-apps/plugin-dialog";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import AskChat from "@/components/AskChat.vue";
import { api, type Scope, type TranscriptSummary } from "@/lib/api";
import { useLibrary } from "@/stores/library";

const route = useRoute();
const router = useRouter();
const library = useLibrary();
const all = ref<TranscriptSummary[]>([]);

/** Scope choices: everything, each folder, each tag. Encoded as strings for the select. */
const key = ref<string>(
  typeof route.query.scope === "string" ? route.query.scope : library.selection.kind === "all" ? "library" : `${library.selection.kind}:${library.selection.id}`,
);
const scope = computed<Scope>(() => {
  const [kind, id] = key.value.split(":");
  if (kind === "folder") return { kind: "folder", id: Number(id) };
  if (kind === "tag") return { kind: "tag", id: Number(id) };
  return { kind: "library" };
});
const titles = computed(() => Object.fromEntries(all.value.map((t) => [t.id, t.title])));
const count = computed(() => {
  const s = scope.value;
  if (s.kind === "library") return all.value.length;
  if (s.kind === "tag") return all.value.filter((t) => t.tags.some((x) => x.id === s.id)).length;
  if (s.kind !== "folder") return 0;
  // Folder counts include sub-folders.
  const ids = new Set<number>([s.id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const f of library.folders) if (f.parent_id !== null && ids.has(f.parent_id) && !ids.has(f.id)) (ids.add(f.id), (grew = true));
  }
  return all.value.filter((t) => t.folder_id !== null && ids.has(t.folder_id)).length;
});
const initial = typeof route.query.q === "string" ? route.query.q : undefined;

onMounted(async () => {
  all.value = await api.listTranscripts({ folder_id: null, tag_id: null, recursive: true });
});

const briefing = ref(false);
async function exportBrief() {
  if (briefing.value) return;
  briefing.value = true;
  toast("Writing the project brief…", { description: "About half a minute; only meetings with a summary are included." });
  try {
    const md = await api.aiBrief(scope.value);
    const path = await save({ defaultPath: "Project brief.md", filters: [{ name: "Markdown", extensions: ["md"] }] });
    if (path) {
      await api.saveText(path, md);
      toast.success("Brief exported");
    }
  } catch (e) {
    toast.error("Brief failed", { description: String(e) });
  } finally {
    briefing.value = false;
  }
}

function open(id: string, time: number) {
  router.push({ name: "transcript", params: { id }, query: { t: String(time) } });
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex items-center gap-3 border-b px-6 py-4" data-tauri-drag-region>
      <Sparkles class="text-primary size-5" />
      <div>
        <h1 class="text-lg font-semibold">Ask</h1>
        <p class="text-muted-foreground text-xs">Questions across meetings, answered from their transcripts with citations.</p>
      </div>
      <div class="ml-auto flex items-center gap-2">
        <span class="text-muted-foreground text-xs">{{ count }} meeting(s)</span>
        <Button variant="outline" size="sm" :disabled="briefing || !count" title="Requirements, decisions, constraints and open questions for developers, with citations" @click="exportBrief">
          <Loader2 v-if="briefing" class="animate-spin" /><FileText v-else /> Export brief
        </Button>
        <Select v-model="key">
          <SelectTrigger class="w-56"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="library">All transcripts</SelectItem>
            <SelectItem v-for="f in library.folders" :key="f.id" :value="`folder:${f.id}`">📁 {{ library.folderPath(f.id) }}</SelectItem>
            <SelectItem v-for="t in library.tags" :key="t.id" :value="`tag:${t.id}`">#{{ t.name }}</SelectItem>
          </SelectContent>
        </Select>
      </div>
    </header>
    <div class="mx-auto flex min-h-0 w-full max-w-3xl flex-1 flex-col">
      <AskChat :scope="scope" :titles="titles" :initial="initial" @open="open" @seek="() => {}" />
    </div>
  </div>
</template>
