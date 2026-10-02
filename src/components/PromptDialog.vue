<script setup lang="ts">
import { ref } from "vue";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const props = defineProps<{ title: string; initial?: string }>();
const emit = defineEmits<{ close: []; submit: [value: string] }>();
const value = ref(props.initial ?? "");

function submit() {
  if (value.value.trim()) emit("submit", value.value.trim());
}
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="sm:max-w-sm">
      <DialogHeader><DialogTitle>{{ title }}</DialogTitle></DialogHeader>
      <form @submit.prevent="submit">
        <Input v-model="value" autofocus placeholder="Name" />
        <DialogFooter class="mt-4">
          <Button type="button" variant="ghost" @click="emit('close')">Cancel</Button>
          <Button type="submit" :disabled="!value.trim()">Save</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
