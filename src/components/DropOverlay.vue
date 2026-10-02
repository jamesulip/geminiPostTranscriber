<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Upload } from "@lucide/vue";
import { toast } from "vue-sonner";
import { useUi } from "@/stores/ui";
import { isAudioPath } from "@/lib/format";

const ui = useUi();
const active = ref(false);
let unlisten: (() => void) | undefined;

onMounted(async () => {
  unlisten = await getCurrentWebview().onDragDropEvent((e) => {
    const p = e.payload;
    if (p.type === "enter" || p.type === "over") active.value = true;
    else if (p.type === "leave") active.value = false;
    else if (p.type === "drop") {
      active.value = false;
      const audio = p.paths.filter(isAudioPath);
      if (audio.length < p.paths.length) toast.warning("Some dropped files aren't audio and were skipped");
      if (audio.length) ui.pendingFiles = audio;
    }
  });
});
onUnmounted(() => unlisten?.());
</script>

<template>
  <Transition
    enter-active-class="transition-opacity duration-150"
    leave-active-class="transition-opacity duration-150"
    enter-from-class="opacity-0"
    leave-to-class="opacity-0"
  >
    <div v-if="active" class="bg-background/80 pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-6 backdrop-blur-sm">
      <div class="border-primary flex h-full w-full flex-col items-center justify-center gap-3 rounded-2xl border-2 border-dashed">
        <Upload class="text-primary size-10" />
        <p class="text-lg font-medium">Drop audio to transcribe</p>
      </div>
    </div>
  </Transition>
</template>
