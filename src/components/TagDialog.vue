<script setup lang="ts">
import { ref } from "vue";
import { Check } from "@lucide/vue";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { TAG_COLORS } from "@/lib/format";

// Name and color of a tag (new or existing).
const props = defineProps<{ title: string; initialName?: string; initialColor?: string }>();
const emit = defineEmits<{ close: []; submit: [name: string, color: string] }>();
const name = ref(props.initialName ?? "");
const color = ref(props.initialColor ?? TAG_COLORS[0]);

function submit() {
  if (name.value.trim()) emit("submit", name.value.trim(), color.value);
}
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="sm:max-w-sm">
      <DialogHeader><DialogTitle>{{ title }}</DialogTitle></DialogHeader>
      <form class="grid gap-4" @submit.prevent="submit">
        <Input v-model="name" autofocus placeholder="Tag name" />
        <div class="flex flex-wrap gap-2">
          <button
            v-for="c in TAG_COLORS"
            :key="c"
            type="button"
            class="flex size-7 items-center justify-center rounded-full text-white ring-offset-2 ring-offset-background transition-shadow"
            :class="color === c && 'ring-2 ring-foreground/60'"
            :style="{ backgroundColor: c }"
            :title="c"
            @click="color = c"
          >
            <Check v-if="color === c" class="size-4" />
          </button>
        </div>
        <span class="rounded-full px-2 py-0.5 text-xs text-white justify-self-start" :style="{ backgroundColor: color }">
          {{ name.trim() || "Preview" }}
        </span>
        <DialogFooter>
          <Button type="button" variant="ghost" @click="emit('close')">Cancel</Button>
          <Button type="submit" :disabled="!name.trim()">Save</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
