<script setup lang="ts">
import { Calendar, Clock, Cpu, Languages } from "@lucide/vue";
import type { TranscriptSummary } from "@/lib/api";
import { formatDate, formatDay, formatDuration, formatLanguage } from "@/lib/format";

defineProps<{ t: TranscriptSummary }>();
</script>

<template>
  <div class="text-muted-foreground flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
    <span class="flex items-center gap-1" title="Recording duration">
      <Clock class="size-3.5" /> <span class="tabular-nums">{{ formatDuration(t.duration) }}</span>
    </span>
    <span
      class="flex items-center gap-1"
      :title="`Recorded ${t.recorded_at ? formatDate(t.recorded_at) : 'unknown'} · transcribed ${formatDate(t.created_at)}`"
    >
      <Calendar class="size-3.5" /> {{ formatDay(t.recorded_at ?? t.created_at) }}
    </span>
    <span class="flex items-center gap-1" title="Language">
      <Languages class="size-3.5" /> {{ formatLanguage(t.language) }}
    </span>
    <span class="flex min-w-0 items-center gap-1" :title="`${t.provider} · ${t.model}`">
      <Cpu class="size-3.5 shrink-0" /> <span class="truncate">{{ t.model }}</span>
    </span>
    <span v-if="t.version > 1" class="bg-muted rounded px-1.5 text-[10px] font-medium" title="Transcription version">v{{ t.version }}</span>
  </div>
</template>
