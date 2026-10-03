<script setup lang="ts">
import { computed } from "vue";
import { AlertCircle, Folder, Loader2, Moon } from "@lucide/vue";
import { Progress } from "@/components/ui/progress";
import type { TranscriptSummary } from "@/lib/api";
import { formatDuration } from "@/lib/format";
import TranscriptMeta from "./TranscriptMeta.vue";
import { useJobs } from "@/stores/jobs";
import { useLibrary } from "@/stores/library";

const props = defineProps<{ t: TranscriptSummary }>();
const jobs = useJobs();
const library = useLibrary();
const job = computed(() => jobs.progress[props.t.id]);
const saved = computed(() =>
  props.t.duration && props.t.processed_duration ? Math.round((1 - props.t.processed_duration / props.t.duration) * 100) : null,
);
</script>

<template>
  <RouterLink
    :to="{ name: 'transcript', params: { id: t.id } }"
    class="hover:border-primary/40 hover:bg-accent/30 bg-card flex flex-col gap-2 rounded-xl border p-4 transition-colors"
  >
    <div class="grid gap-1">
      <h3 class="line-clamp-1 font-medium">{{ t.title }}</h3>
      <TranscriptMeta :t="t" />
    </div>

    <div v-if="t.status === 'queued'" class="text-muted-foreground flex items-center gap-1.5 text-xs">
      <Moon class="size-3.5" /> {{ job?.message ?? "Overnight batch — waiting at Google" }}
    </div>
    <div v-else-if="t.status === 'processing'" class="grid gap-1.5">
      <div class="text-muted-foreground flex items-center gap-1.5 text-xs">
        <Loader2 class="size-3.5 animate-spin" /> {{ job?.message ?? "Processing…" }}
      </div>
      <Progress :model-value="(job?.progress ?? 0.02) * 100" class="h-1.5" />
    </div>
    <p v-else-if="t.status === 'error'" class="text-destructive flex items-start gap-1.5 text-xs">
      <AlertCircle class="mt-px size-3.5 shrink-0" /> <span class="line-clamp-2">{{ t.error }}</span>
    </p>
    <p v-else class="text-muted-foreground line-clamp-2 text-sm">{{ t.preview || "No speech detected" }}</p>

    <div class="text-muted-foreground mt-auto flex flex-wrap items-center gap-1.5 pt-1 text-xs">
      <span v-if="t.folder_id" class="flex items-center gap-1"><Folder class="size-3" /> {{ library.folderPath(t.folder_id) }}</span>
      <span
        v-for="tag in t.tags"
        :key="tag.id"
        class="rounded-full px-2 py-0.5 text-[11px] text-white"
        :style="{ backgroundColor: tag.color }"
      >{{ tag.name }}</span>
    </div>
    <div v-if="saved !== null && t.status === 'done'" class="text-muted-foreground text-[11px]">
      {{ t.provider }} · sent {{ formatDuration(t.processed_duration) }} ({{ saved }}% less)
    </div>
  </RouterLink>
</template>
