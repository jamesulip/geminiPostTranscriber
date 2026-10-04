<script setup lang="ts">
import { computed } from "vue";
import { AlertCircle, AlertTriangle, Calendar, Clock, Folder, Loader2, Moon } from "@lucide/vue";
import type { TranscriptSummary } from "@/lib/api";
import { formatDay, formatDuration } from "@/lib/format";
import { fixReason, needsFix } from "@/lib/transcripts";
import TranscriptActions from "./TranscriptActions.vue";
import { useJobs } from "@/stores/jobs";
import { useLibrary } from "@/stores/library";

// One compact line per transcript (the library's list view).
const props = defineProps<{ t: TranscriptSummary }>();
const jobs = useJobs();
const library = useLibrary();
const job = computed(() => jobs.progress[props.t.id]);
</script>

<template>
  <RouterLink
    :to="{ name: 'transcript', params: { id: t.id } }"
    class="group hover:bg-accent/40 flex items-center gap-3 border-b px-3 py-2 text-sm last:border-b-0"
  >
    <div class="flex min-w-0 flex-1 items-center gap-2">
      <span class="truncate font-medium">{{ t.title }}</span>
      <span
        v-if="needsFix(t)"
        class="flex shrink-0 items-center gap-1 rounded-full bg-amber-100 px-2 py-0.5 text-[11px] font-medium text-amber-800 dark:bg-amber-950/60 dark:text-amber-400"
        :title="fixReason(t)"
      >
        <AlertTriangle class="size-3" /> Needs fix
      </span>
      <span v-if="t.folder_id" class="text-muted-foreground flex shrink-0 items-center gap-1 text-xs">
        <Folder class="size-3" /> {{ library.folderPath(t.folder_id) }}
      </span>
    </div>

    <div class="hidden min-w-0 max-w-56 flex-wrap items-center justify-end gap-1 md:flex">
      <span
        v-for="tag in t.tags"
        :key="tag.id"
        class="rounded-full px-2 py-0.5 text-[11px] text-white"
        :style="{ backgroundColor: tag.color }"
      >{{ tag.name }}</span>
    </div>

    <span v-if="t.status === 'queued'" class="text-muted-foreground flex w-36 shrink-0 items-center gap-1.5 text-xs" :title="job?.message">
      <Moon class="size-3.5" /> Overnight batch
    </span>
    <span v-else-if="t.status === 'processing'" class="text-muted-foreground flex w-36 shrink-0 items-center gap-1.5 truncate text-xs" :title="job?.message">
      <Loader2 class="size-3.5 shrink-0 animate-spin" /> {{ job?.message ?? "Processing…" }}
    </span>
    <span v-else-if="t.status === 'error'" class="text-destructive flex w-36 shrink-0 items-center gap-1.5 truncate text-xs" :title="t.error ?? ''">
      <AlertCircle class="size-3.5 shrink-0" /> Failed
    </span>
    <span v-else class="text-muted-foreground w-36 shrink-0 truncate text-xs" :title="t.model">{{ t.model }}</span>

    <span class="text-muted-foreground flex w-16 shrink-0 items-center gap-1 text-xs tabular-nums" title="Recording duration">
      <Clock class="size-3.5" /> {{ formatDuration(t.duration) }}
    </span>
    <span class="text-muted-foreground flex w-28 shrink-0 items-center gap-1 text-xs" :title="t.trashed_at ? 'Trashed' : 'Recorded'">
      <Calendar class="size-3.5" /> {{ formatDay(t.trashed_at ?? t.recorded_at ?? t.created_at) }}
    </span>
    <TranscriptActions :t="t" />
  </RouterLink>
</template>
