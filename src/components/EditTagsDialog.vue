<script setup lang="ts">
import { ref } from "vue";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import TagPicker from "./TagPicker.vue";
import type { TranscriptSummary } from "@/lib/api";
import { useLibrary } from "@/stores/library";

// Change a transcript's tags from the library, without opening it.
const props = defineProps<{ t: TranscriptSummary }>();
const emit = defineEmits<{ close: [] }>();
const library = useLibrary();
const tagIds = ref(props.t.tags.map((g) => g.id));
const saving = ref(false);

async function save() {
  saving.value = true;
  try {
    await library.setTags(props.t.id, tagIds.value);
    emit("close");
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="sm:max-w-sm">
      <DialogHeader>
        <DialogTitle>Edit tags</DialogTitle>
        <DialogDescription class="truncate">{{ t.title }}</DialogDescription>
      </DialogHeader>
      <TagPicker v-model="tagIds" />
      <DialogFooter class="mt-2">
        <Button variant="ghost" @click="emit('close')">Cancel</Button>
        <Button :disabled="saving" @click="save">Save</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
