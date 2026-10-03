<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { AlertTriangle, Loader2, RefreshCw, Sparkles } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import AskChat from "./AskChat.vue";
import { api, type Digest, type TranscriptDetail } from "@/lib/api";
import { formatDate, formatTime } from "@/lib/format";
import { renderMarkdown } from "@/lib/markdown";
import { useAi } from "@/stores/ai";
import { useSettings } from "@/stores/settings";

const props = defineProps<{ t: TranscriptDetail; names: Record<string, string>; readOnly?: boolean }>();
const emit = defineEmits<{ seek: [time: number]; open: [id: string, time: number]; refresh: []; close: [] }>();

const ai = useAi();
const settingsStore = useSettings();
const tab = ref<"summary" | "ask">("summary");
const digests = ref<Digest[]>([]);
const key = computed(() => `digest:${props.t.id}`);
const running = computed(() => ai.running[key.value]);
const configured = computed(() => {
  const s = settingsStore.settings;
  return !!s && !!s.providers.find((p) => p.id === s.ai_provider)?.api_key.trim();
});

async function load() {
  digests.value = await api.aiGetDigests(props.t.id);
}
onMounted(load);
watch(() => ai.digestVersion[props.t.id], async () => {
  const before = summary.value?.content_hash;
  await load();
  if (summary.value?.content_hash !== before) emit("refresh");
});
watch(() => props.t.content_hash, load);

const by = (kind: string) => digests.value.find((d) => d.kind === kind);
const summary = computed(() => by("summary"));
const stale = computed(() => !!summary.value?.stale);
function list<T>(kind: string): T[] {
  try {
    const v = JSON.parse(by(kind)?.content ?? "[]");
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
}
type Timed = { text: string; time: number | null };
type Action = { owner: string | null; task: string; due: string | null; time: number | null };
type Topic = { title: string; start: number; end: number };
type Term = { term: string; meaning: string };
type SpeakerGuess = { label: string; name: string; evidence: string };
const decisions = computed(() => list<Timed>("decisions"));
const actions = computed(() => list<Action>("actions"));
const questions = computed(() => list<Timed>("questions"));
const topics = computed(() => list<Topic>("topics"));
const glossary = computed(() => list<Term>("glossary"));
const speakerGuesses = computed(() =>
  list<SpeakerGuess>("speakers").filter((g) => g.name && props.names[g.label] !== undefined && props.names[g.label] === g.label),
);

async function rename(g: SpeakerGuess) {
  await api.renameSpeaker(props.t.id, g.label, g.name);
  toast.success(`${g.label} is now ${g.name}`);
  emit("refresh");
}

const seekable = (time: number | null | undefined) => typeof time === "number" && Number.isFinite(time);
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div class="flex items-center gap-1 border-b px-2 py-1.5">
      <button
        v-for="x in [{ id: 'summary', label: 'Summary' }, { id: 'ask', label: 'Ask' }] as const"
        :key="x.id"
        class="rounded-md px-2.5 py-1 text-sm"
        :class="tab === x.id ? 'bg-accent font-medium' : 'text-muted-foreground hover:bg-accent/50'"
        @click="tab = x.id"
      >{{ x.label }}</button>
      <span class="flex-1" />
      <Button
        v-if="tab === 'summary' && summary && !running"
        variant="ghost"
        size="sm"
        class="text-muted-foreground"
        :title="stale ? 'The transcript changed since this summary was made' : 'Redo the summary'"
        @click="ai.digest(t.id, true)"
      ><RefreshCw /> {{ stale ? "Update" : "Redo" }}</Button>
      <Button variant="ghost" size="icon" class="size-7" title="Close" @click="emit('close')">×</Button>
    </div>

    <div v-if="tab === 'summary'" class="min-h-0 flex-1 overflow-y-auto px-4 py-3 text-sm" @click="(e) => { const b = (e.target as HTMLElement).closest<HTMLElement>('[data-time]'); if (b) emit('seek', parseFloat(b.dataset.time!)); }">
      <div v-if="running" class="grid gap-2 py-6 text-center">
        <Loader2 class="text-primary mx-auto size-6 animate-spin" />
        <p>{{ running.message }}</p>
        <Progress :model-value="running.progress * 100" class="mx-auto h-1.5 w-56" />
        <Button variant="ghost" size="sm" class="mx-auto" @click="ai.cancel(key)">Cancel</Button>
      </div>
      <div v-else-if="!summary" class="grid gap-3 py-6 text-center">
        <Sparkles class="text-primary mx-auto size-6" />
        <p>No summary yet.</p>
        <p class="text-muted-foreground text-xs">
          Summarises each part once, then writes decisions, action items, open questions, topics and a glossary. Costs cents for a long meeting; it isn't redone unless the transcript changes.
        </p>
        <Button class="mx-auto" :disabled="!configured || !t.segments.length" @click="ai.digest(t.id)"><Sparkles /> Summarise this meeting</Button>
        <p v-if="!configured" class="text-muted-foreground text-xs">Set an AI provider and key under Settings → AI assistant first.</p>
      </div>
      <template v-else>
        <div v-if="stale" class="mb-3 flex items-center gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs dark:border-amber-900 dark:bg-amber-950/40">
          <AlertTriangle class="size-3.5 shrink-0 text-amber-600" /> The transcript changed since this summary was made.
          <Button size="sm" variant="outline" class="ml-auto h-6" @click="ai.digest(t.id)">Update</Button>
        </div>
        <div class="prose-ai" v-html="renderMarkdown(summary.content)" />

        <section v-if="topics.length" class="mt-4">
          <h4 class="text-muted-foreground mb-1 text-xs font-medium uppercase">Topics</h4>
          <button v-for="(x, i) in topics" :key="i" class="hover:bg-accent flex w-full items-center gap-2 rounded-md px-2 py-1 text-left" :data-time="x.start">
            <span class="text-muted-foreground w-14 shrink-0 text-xs tabular-nums">{{ formatTime(x.start) }}</span>
            <span class="truncate">{{ x.title }}</span>
          </button>
        </section>

        <section v-if="decisions.length" class="mt-4">
          <h4 class="text-muted-foreground mb-1 text-xs font-medium uppercase">Decisions</h4>
          <ul class="grid gap-1">
            <li v-for="(x, i) in decisions" :key="i" class="flex items-start gap-2">
              <button v-if="seekable(x.time)" class="cite mt-0.5 shrink-0" :data-time="x.time!">{{ formatTime(x.time!) }}</button>
              <span>{{ x.text }}</span>
            </li>
          </ul>
        </section>

        <section v-if="actions.length" class="mt-4">
          <h4 class="text-muted-foreground mb-1 text-xs font-medium uppercase">Action items</h4>
          <ul class="grid gap-1">
            <li v-for="(x, i) in actions" :key="i" class="flex items-start gap-2">
              <button v-if="seekable(x.time)" class="cite mt-0.5 shrink-0" :data-time="x.time!">{{ formatTime(x.time!) }}</button>
              <span><b v-if="x.owner">{{ x.owner }}: </b>{{ x.task }}<span v-if="x.due" class="text-muted-foreground"> · {{ x.due }}</span></span>
            </li>
          </ul>
        </section>

        <section v-if="questions.length" class="mt-4">
          <h4 class="text-muted-foreground mb-1 text-xs font-medium uppercase">Open questions</h4>
          <ul class="grid gap-1">
            <li v-for="(x, i) in questions" :key="i" class="flex items-start gap-2">
              <button v-if="seekable(x.time)" class="cite mt-0.5 shrink-0" :data-time="x.time!">{{ formatTime(x.time!) }}</button>
              <span>{{ x.text }}</span>
            </li>
          </ul>
        </section>

        <section v-if="speakerGuesses.length && !readOnly" class="mt-4">
          <h4 class="text-muted-foreground mb-1 text-xs font-medium uppercase">Who is who</h4>
          <ul class="grid gap-1">
            <li v-for="g in speakerGuesses" :key="g.label" class="flex items-center gap-2">
              <span class="min-w-0 flex-1 truncate" :title="g.evidence">{{ g.label }} is probably <b>{{ g.name }}</b></span>
              <Button size="sm" variant="outline" class="h-6" @click="rename(g)">Rename</Button>
            </li>
          </ul>
        </section>

        <section v-if="glossary.length" class="mt-4">
          <h4 class="text-muted-foreground mb-1 text-xs font-medium uppercase">Glossary</h4>
          <dl class="grid gap-1">
            <div v-for="(x, i) in glossary" :key="i"><dt class="inline font-medium">{{ x.term }}</dt><dd class="text-muted-foreground inline"> — {{ x.meaning }}</dd></div>
          </dl>
        </section>

        <p class="text-muted-foreground mt-6 text-[11px]">{{ summary.model }} · {{ formatDate(summary.created_at) }}</p>
      </template>
    </div>

    <AskChat
      v-else
      class="min-h-0 flex-1"
      :scope="{ kind: 'transcript', id: t.id }"
      :current-id="t.id"
      allow-read-all
      @seek="(time) => emit('seek', time)"
      @open="(id, time) => emit('open', id, time)"
    />
  </div>
</template>
