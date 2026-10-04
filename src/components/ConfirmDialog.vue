<script setup lang="ts">
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";

defineProps<{ title: string; description?: string; confirmLabel?: string; destructive?: boolean }>();
const emit = defineEmits<{ close: []; confirm: [] }>();
</script>

<template>
  <Dialog :open="true" @update:open="(o) => !o && emit('close')">
    <DialogContent class="sm:max-w-sm">
      <DialogHeader>
        <DialogTitle>{{ title }}</DialogTitle>
        <DialogDescription v-if="description">{{ description }}</DialogDescription>
      </DialogHeader>
      <DialogFooter class="mt-2">
        <Button variant="ghost" @click="emit('close')">Cancel</Button>
        <Button :variant="destructive ? 'destructive' : 'default'" autofocus @click="emit('confirm')">{{ confirmLabel ?? "OK" }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
