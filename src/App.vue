<script setup lang="ts">
import { onMounted } from "vue";
import { Toaster } from "@/components/ui/sonner";
import AppSidebar from "@/components/AppSidebar.vue";
import DropOverlay from "@/components/DropOverlay.vue";
import NewTranscriptionDialog from "@/components/NewTranscriptionDialog.vue";
import SearchDialog from "@/components/SearchDialog.vue";
import { useLibrary } from "@/stores/library";
import { useJobs } from "@/stores/jobs";
import { useSettings } from "@/stores/settings";
import { useAi } from "@/stores/ai";
import { useUpdates } from "@/stores/updates";

const library = useLibrary();
const jobs = useJobs();
const settings = useSettings();
const ai = useAi();
const updates = useUpdates();

onMounted(async () => {
  await Promise.all([library.refreshMeta(), library.refresh(), jobs.start(), settings.load(), ai.start(), updates.start()]);
});
</script>

<template>
  <div class="bg-background text-foreground flex h-screen overflow-hidden">
    <AppSidebar />
    <main class="min-w-0 flex-1">
      <RouterView :key="$route.name === 'transcript' ? 'transcript' : String($route.name)" />
    </main>
    <DropOverlay />
    <NewTranscriptionDialog />
    <SearchDialog />
    <Toaster rich-colors position="bottom-right" />
  </div>
</template>
