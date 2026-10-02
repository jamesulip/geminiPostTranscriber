<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { FileAudio } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import OptionsForm from "./OptionsForm.vue";
import FolderSelect from "./FolderSelect.vue";
import TagPicker from "./TagPicker.vue";
import { api, type JobOptions } from "@/lib/api";
import { defaultOptions, normalizeOptions } from "@/lib/options";
import { useUi } from "@/stores/ui";
import { useSettings } from "@/stores/settings";
import { useLibrary } from "@/stores/library";

const ui = useUi();
const settingsStore = useSettings();
const library = useLibrary();
const router = useRouter();
const opts = ref<JobOptions | null>(null);
const busy = ref(false);

const open = computed({
  get: () => ui.pendingFiles.length > 0 && !!opts.value,
  set: (v) => !v && (ui.pendingFiles = []),
});
const files = computed(() => ui.pendingFiles);
const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;

watch(
  () => ui.pendingFiles.length,
  async (n) => {
    if (!n) return;
    const s = await settingsStore.load();
    const o = defaultOptions(s);
    // Default the folder to whatever is selected in the sidebar.
    if (library.selection.kind === "folder") o.folder_id = library.selection.id;
    if (library.selection.kind === "tag") o.tag_ids = [library.selection.id];
    opts.value = o;
  },
);

async function start() {
  if (!opts.value) return;
  busy.value = true;
  try {
    const base = normalizeOptions(opts.value);
    const list = [...files.value];
    const ids: string[] = [];
    for (const f of list) ids.push(await api.startTranscription(f, list.length === 1 ? base : { ...base, title: null }));
    ui.pendingFiles = [];
    await library.refresh();
    router.push(ids.length === 1 ? { name: "transcript", params: { id: ids[0] } } : { name: "library" });
  } catch (e) {
    toast.error("Couldn't start transcription", { description: String(e) });
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="max-h-[90vh] overflow-y-auto sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>New transcription</DialogTitle>
        <DialogDescription>Audio is condensed locally before upload to reduce cost.</DialogDescription>
      </DialogHeader>
      <div v-if="opts && settingsStore.settings" class="grid gap-4">
        <div class="bg-muted/50 max-h-28 overflow-y-auto rounded-lg p-2">
          <div v-for="f in files" :key="f" class="flex items-center gap-2 px-1 py-0.5 text-sm">
            <FileAudio class="text-muted-foreground size-4 shrink-0" />
            <span class="truncate" :title="f">{{ fileName(f) }}</span>
          </div>
        </div>
        <div v-if="files.length === 1" class="grid gap-1.5">
          <Label>Title</Label>
          <Input v-model="opts.title as string" :placeholder="fileName(files[0]).replace(/\.[^.]+$/, '')" />
        </div>
        <div class="grid grid-cols-2 gap-3">
          <div class="grid gap-1.5"><Label>Folder</Label><FolderSelect v-model="opts.folder_id" /></div>
          <div class="grid gap-1.5"><Label>Tags</Label><TagPicker v-model="opts.tag_ids" /></div>
        </div>
        <OptionsForm v-model="opts" :settings="settingsStore.settings" />
      </div>
      <DialogFooter>
        <Button variant="ghost" @click="open = false">Cancel</Button>
        <Button :disabled="busy" @click="start">
          Transcribe {{ files.length > 1 ? `${files.length} files` : "" }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
