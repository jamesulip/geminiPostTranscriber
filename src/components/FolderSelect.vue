<script setup lang="ts">
import { computed } from "vue";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useLibrary } from "@/stores/library";

const model = defineModel<number | null>({ required: true });
const library = useLibrary();

// reka Select needs string values; "none" = no folder.
const value = computed({
  get: () => (model.value == null ? "none" : String(model.value)),
  set: (v: string) => (model.value = v === "none" ? null : Number(v)),
});
const options = computed(() =>
  library.folders
    .map((f) => ({ id: f.id, label: library.folderPath(f.id) }))
    .sort((a, b) => a.label.localeCompare(b.label)),
);
</script>

<template>
  <Select v-model="value">
    <SelectTrigger class="w-full"><SelectValue placeholder="No folder" /></SelectTrigger>
    <SelectContent>
      <SelectItem value="none">No folder</SelectItem>
      <SelectItem v-for="o in options" :key="o.id" :value="String(o.id)">{{ o.label }}</SelectItem>
    </SelectContent>
  </Select>
</template>
