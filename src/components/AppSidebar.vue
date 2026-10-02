<script setup lang="ts">
import { ref } from "vue";
import { useRouter, useRoute } from "vue-router";
import {
  AudioLines, ChevronRight, Folder, FolderPlus, Inbox, MoreHorizontal, Plus, Search, Settings, Tag as TagIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import PromptDialog from "./PromptDialog.vue";
import { useLibrary, type Selection } from "@/stores/library";
import { useUi } from "@/stores/ui";
import { useJobs } from "@/stores/jobs";
import { TAG_COLORS } from "@/lib/format";
import { pickAudioFiles } from "@/lib/files";
import type { Folder as FolderT } from "@/lib/api";

const library = useLibrary();
const ui = useUi();
const jobs = useJobs();
const router = useRouter();
const route = useRoute();
const collapsed = ref<Set<number>>(new Set());

type Prompt = { title: string; initial?: string; submit: (v: string) => Promise<unknown> };
const prompt = ref<Prompt | null>(null);

function isSelected(s: Selection) {
  const cur = library.selection;
  if (route.name !== "library") return false;
  return cur.kind === s.kind && (s.kind === "all" || (cur as { id: number }).id === (s as { id: number }).id);
}

async function go(s: Selection) {
  await library.select(s);
  if (route.name !== "library") router.push({ name: "library" });
}

function children(parent: number | null): FolderT[] {
  return library.folders.filter((f) => f.parent_id === parent);
}

function toggle(id: number) {
  const s = new Set(collapsed.value);
  s.has(id) ? s.delete(id) : s.add(id);
  collapsed.value = s;
}

function flatTree(): { folder: FolderT; depth: number; hasChildren: boolean }[] {
  const out: { folder: FolderT; depth: number; hasChildren: boolean }[] = [];
  const walk = (parent: number | null, depth: number) => {
    for (const f of children(parent)) {
      const hasChildren = children(f.id).length > 0;
      out.push({ folder: f, depth, hasChildren });
      if (!collapsed.value.has(f.id)) walk(f.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

function newFolder(parentId: number | null = null) {
  prompt.value = { title: parentId ? "New sub-folder" : "New folder", submit: (n) => library.createFolder(n, parentId) };
}

function newTag() {
  prompt.value = {
    title: "New tag",
    submit: (n) => library.createTag(n, TAG_COLORS[library.tags.length % TAG_COLORS.length]),
  };
}

async function upload() {
  const files = await pickAudioFiles();
  if (files.length) ui.pendingFiles = files;
}
</script>

<template>
  <aside class="bg-sidebar text-sidebar-foreground border-sidebar-border flex h-full w-60 shrink-0 flex-col border-r">
    <div class="flex items-center gap-2 px-4 pt-4 pb-3" data-tauri-drag-region>
      <AudioLines class="text-primary size-5" />
      <span class="font-semibold">Transcriber</span>
    </div>

    <div class="flex flex-col gap-1.5 px-3">
      <Button class="w-full justify-start" @click="upload"><Plus /> New transcription</Button>
      <Button variant="outline" class="text-muted-foreground w-full justify-start" @click="ui.searchOpen = true">
        <Search /> Search
        <kbd class="bg-muted ml-auto rounded px-1.5 text-[10px]">⌘K</kbd>
      </Button>
    </div>

    <ScrollArea class="mt-3 min-h-0 flex-1">
      <nav class="flex flex-col gap-0.5 px-3 pb-4 text-sm">
        <button
          class="hover:bg-sidebar-accent flex items-center gap-2 rounded-md px-2 py-1.5 text-left"
          :class="isSelected({ kind: 'all' }) && 'bg-sidebar-accent font-medium'"
          @click="go({ kind: 'all' })"
        >
          <Inbox class="size-4" /> All transcripts
          <span v-if="Object.keys(jobs.progress).length" class="bg-primary text-primary-foreground ml-auto rounded-full px-1.5 text-[10px]">
            {{ Object.keys(jobs.progress).length }}
          </span>
        </button>

        <div class="text-muted-foreground mt-4 mb-1 flex items-center justify-between px-2 text-xs font-medium uppercase">
          Folders
          <button class="hover:text-foreground" title="New folder" @click="newFolder()"><FolderPlus class="size-3.5" /></button>
        </div>
        <div
          v-for="{ folder, depth, hasChildren } in flatTree()"
          :key="folder.id"
          class="group hover:bg-sidebar-accent flex items-center rounded-md"
          :class="isSelected({ kind: 'folder', id: folder.id }) && 'bg-sidebar-accent font-medium'"
          :style="{ paddingLeft: `${depth * 12}px` }"
        >
          <button class="text-muted-foreground w-5 shrink-0 pl-1" :class="!hasChildren && 'invisible'" @click="toggle(folder.id)">
            <ChevronRight class="size-3.5 transition-transform" :class="!collapsed.has(folder.id) && 'rotate-90'" />
          </button>
          <button class="flex min-w-0 flex-1 items-center gap-2 py-1.5 pr-1 text-left" @click="go({ kind: 'folder', id: folder.id })">
            <Folder class="size-4 shrink-0" /> <span class="truncate">{{ folder.name }}</span>
          </button>
          <DropdownMenu>
            <DropdownMenuTrigger as-child>
              <button class="text-muted-foreground mr-1 opacity-0 group-hover:opacity-100"><MoreHorizontal class="size-4" /></button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              <DropdownMenuItem @click="newFolder(folder.id)">New sub-folder</DropdownMenuItem>
              <DropdownMenuItem
                @click="prompt = { title: 'Rename folder', initial: folder.name, submit: (n) => library.renameFolder(folder.id, n) }"
              >Rename</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" @click="library.deleteFolder(folder.id)">Delete</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <p v-if="!library.folders.length" class="text-muted-foreground px-2 py-1 text-xs">No folders yet</p>

        <div class="text-muted-foreground mt-4 mb-1 flex items-center justify-between px-2 text-xs font-medium uppercase">
          Tags
          <button class="hover:text-foreground" title="New tag" @click="newTag"><Plus class="size-3.5" /></button>
        </div>
        <div
          v-for="tag in library.tags"
          :key="tag.id"
          class="group hover:bg-sidebar-accent flex items-center rounded-md"
          :class="isSelected({ kind: 'tag', id: tag.id }) && 'bg-sidebar-accent font-medium'"
        >
          <button class="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left" @click="go({ kind: 'tag', id: tag.id })">
            <TagIcon class="size-3.5 shrink-0" :style="{ color: tag.color }" /> <span class="truncate">{{ tag.name }}</span>
          </button>
          <DropdownMenu>
            <DropdownMenuTrigger as-child>
              <button class="text-muted-foreground mr-1 opacity-0 group-hover:opacity-100"><MoreHorizontal class="size-4" /></button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              <DropdownMenuItem variant="destructive" @click="library.deleteTag(tag.id)">Delete tag</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <p v-if="!library.tags.length" class="text-muted-foreground px-2 py-1 text-xs">No tags yet</p>
      </nav>
    </ScrollArea>

    <div class="border-sidebar-border border-t p-3">
      <Button
        variant="ghost"
        class="w-full justify-start"
        :class="route.name === 'settings' && 'bg-sidebar-accent'"
        @click="router.push({ name: 'settings' })"
      >
        <Settings /> Settings
      </Button>
    </div>

    <PromptDialog
      v-if="prompt"
      :title="prompt.title"
      :initial="prompt.initial"
      @close="prompt = null"
      @submit="(v) => prompt!.submit(v).then(() => (prompt = null))"
    />
  </aside>
</template>
