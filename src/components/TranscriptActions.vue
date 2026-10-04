<script setup lang="ts">
import { ref } from "vue";
import { Flag, MoreHorizontal, RotateCcw, Tag as TagIcon, Trash2 } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import ConfirmDialog from "./ConfirmDialog.vue";
import EditTagsDialog from "./EditTagsDialog.vue";
import type { TranscriptSummary } from "@/lib/api";
import { useLibrary } from "@/stores/library";

// Flag toggle + "…" menu shared by the library card and list row. Lives inside a RouterLink,
// so every click stops there instead of opening the transcript.
const props = defineProps<{ t: TranscriptSummary }>();
const library = useLibrary();
const editingTags = ref(false);
const confirmDelete = ref(false);
const inTrash = () => !!props.t.trashed_at;

async function run(action: () => Promise<void>, done?: string) {
  try {
    await action();
    if (done) toast.success(done);
  } catch (e) {
    toast.error("Something went wrong", { description: String(e) });
  }
}
</script>

<template>
  <div class="flex items-center" @click.prevent.stop>
    <Button
      v-if="!inTrash()"
      variant="ghost"
      size="icon-sm"
      :class="t.flagged ? 'text-amber-500 hover:text-amber-600' : 'text-muted-foreground opacity-0 group-hover:opacity-100 focus-visible:opacity-100'"
      :title="t.flagged ? 'Remove flag' : 'Flag for attention'"
      @click="run(() => library.setFlagged(t.id, !t.flagged))"
    >
      <Flag :fill="t.flagged ? 'currentColor' : 'none'" />
    </Button>
    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button
          variant="ghost"
          size="icon-sm"
          class="text-muted-foreground opacity-0 group-hover:opacity-100 focus-visible:opacity-100 aria-expanded:opacity-100"
          title="More"
        >
          <MoreHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <template v-if="inTrash()">
          <DropdownMenuItem @click="run(() => library.restore(t.id), 'Restored')"><RotateCcw /> Restore</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" @click="confirmDelete = true"><Trash2 /> Delete forever…</DropdownMenuItem>
        </template>
        <template v-else>
          <DropdownMenuItem @click="run(() => library.setFlagged(t.id, !t.flagged))">
            <Flag /> {{ t.flagged ? "Remove flag" : "Flag" }}
          </DropdownMenuItem>
          <DropdownMenuItem @click="editingTags = true"><TagIcon /> Edit tags…</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" @click="run(() => library.trash(t.id), 'Moved to trash')">
            <Trash2 /> Move to trash
          </DropdownMenuItem>
        </template>
      </DropdownMenuContent>
    </DropdownMenu>

    <EditTagsDialog v-if="editingTags" :t="t" @close="editingTags = false" />
    <ConfirmDialog
      v-if="confirmDelete"
      title="Delete forever?"
      :description="`“${t.title}” and its audio copy will be removed. This can't be undone.`"
      confirm-label="Delete forever"
      destructive
      @close="confirmDelete = false"
      @confirm="confirmDelete = false; run(() => library.deleteForever(t.id), 'Deleted')"
    />
  </div>
</template>
