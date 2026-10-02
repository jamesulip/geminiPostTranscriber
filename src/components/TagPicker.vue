<script setup lang="ts">
import { computed, ref } from "vue";
import { Check, Plus, Tag as TagIcon } from "@lucide/vue";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useLibrary } from "@/stores/library";
import { TAG_COLORS } from "@/lib/format";

const model = defineModel<number[]>({ required: true });
const library = useLibrary();
const query = ref("");

const filtered = computed(() =>
  library.tags.filter((t) => t.name.toLowerCase().includes(query.value.trim().toLowerCase())),
);
const canCreate = computed(
  () => query.value.trim() && !library.tags.some((t) => t.name.toLowerCase() === query.value.trim().toLowerCase()),
);
const selected = computed(() => library.tags.filter((t) => model.value.includes(t.id)));

function toggle(id: number) {
  model.value = model.value.includes(id) ? model.value.filter((x) => x !== id) : [...model.value, id];
}

async function create() {
  const t = await library.createTag(query.value.trim(), TAG_COLORS[library.tags.length % TAG_COLORS.length]);
  model.value = [...model.value, t.id];
  query.value = "";
}
</script>

<template>
  <Popover>
    <PopoverTrigger as-child>
      <Button variant="outline" class="h-auto min-h-9 w-full justify-start gap-1.5 py-1.5 font-normal">
        <TagIcon class="text-muted-foreground" />
        <template v-if="selected.length">
          <span
            v-for="t in selected"
            :key="t.id"
            class="rounded-full px-2 py-0.5 text-xs text-white"
            :style="{ backgroundColor: t.color }"
          >{{ t.name }}</span>
        </template>
        <span v-else class="text-muted-foreground">Add tags</span>
      </Button>
    </PopoverTrigger>
    <PopoverContent class="w-64 p-2" align="start">
      <Input v-model="query" placeholder="Find or create tag" class="mb-2 h-8" @keydown.enter.prevent="canCreate && create()" />
      <div class="max-h-56 overflow-y-auto">
        <button
          v-for="t in filtered"
          :key="t.id"
          class="hover:bg-accent flex w-full items-center gap-2 rounded px-2 py-1.5 text-sm"
          @click="toggle(t.id)"
        >
          <span class="size-2.5 rounded-full" :style="{ backgroundColor: t.color }" />
          {{ t.name }}
          <Check v-if="model.includes(t.id)" class="ml-auto size-4" />
        </button>
        <button v-if="canCreate" class="hover:bg-accent flex w-full items-center gap-2 rounded px-2 py-1.5 text-sm" @click="create">
          <Plus class="size-4" /> Create “{{ query.trim() }}”
        </button>
        <p v-if="!filtered.length && !canCreate" class="text-muted-foreground px-2 py-1.5 text-sm">No tags</p>
      </div>
    </PopoverContent>
  </Popover>
</template>
