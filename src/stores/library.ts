import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { api, type Folder, type Tag, type TranscriptSummary } from "@/lib/api";

export type Selection = { kind: "all" } | { kind: "folder"; id: number } | { kind: "tag"; id: number };

export const useLibrary = defineStore("library", () => {
  const folders = ref<Folder[]>([]);
  const tags = ref<Tag[]>([]);
  const transcripts = ref<TranscriptSummary[]>([]);
  const selection = ref<Selection>({ kind: "all" });
  const loading = ref(false);

  const selectionLabel = computed(() => {
    const s = selection.value;
    if (s.kind === "folder") return folders.value.find((f) => f.id === s.id)?.name ?? "Folder";
    if (s.kind === "tag") return `#${tags.value.find((t) => t.id === s.id)?.name ?? "tag"}`;
    return "All transcripts";
  });

  async function refreshMeta() {
    [folders.value, tags.value] = await Promise.all([api.listFolders(), api.listTags()]);
  }

  async function refresh() {
    loading.value = true;
    try {
      const s = selection.value;
      transcripts.value = await api.listTranscripts({
        folder_id: s.kind === "folder" ? s.id : null,
        tag_id: s.kind === "tag" ? s.id : null,
        recursive: true,
      });
    } finally {
      loading.value = false;
    }
  }

  async function select(s: Selection) {
    selection.value = s;
    await refresh();
  }

  async function createFolder(name: string, parentId: number | null = null) {
    const f = await api.createFolder(name, parentId);
    await refreshMeta();
    return f;
  }

  async function renameFolder(id: number, name: string) {
    await api.renameFolder(id, name);
    await refreshMeta();
  }

  async function deleteFolder(id: number) {
    await api.deleteFolder(id);
    if (selection.value.kind === "folder" && selection.value.id === id) selection.value = { kind: "all" };
    await Promise.all([refreshMeta(), refresh()]);
  }

  async function createTag(name: string, color: string) {
    const t = await api.createTag(name, color);
    await refreshMeta();
    return t;
  }

  async function deleteTag(id: number) {
    await api.deleteTag(id);
    if (selection.value.kind === "tag" && selection.value.id === id) selection.value = { kind: "all" };
    await Promise.all([refreshMeta(), refresh()]);
  }

  /** Folder path like "Work / Meetings" for display. */
  function folderPath(id: number | null): string {
    const parts: string[] = [];
    let cur = folders.value.find((f) => f.id === id);
    while (cur) {
      parts.unshift(cur.name);
      cur = folders.value.find((f) => f.id === cur!.parent_id);
    }
    return parts.join(" / ");
  }

  return {
    folders, tags, transcripts, selection, selectionLabel, loading,
    refresh, refreshMeta, select, createFolder, renameFolder, deleteFolder, createTag, deleteTag, folderPath,
  };
});
