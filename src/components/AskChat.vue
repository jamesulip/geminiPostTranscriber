<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { BookOpen, ChevronDown, Loader2, MessageSquarePlus, Send, Square, Trash2 } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { api, type AiMessage, type AiThread, type Scope } from "@/lib/api";
import { formatDate, formatTokens, formatUsd } from "@/lib/format";
import { citationFromClick, renderMarkdown } from "@/lib/markdown";
import { useAi } from "@/stores/ai";

const props = defineProps<{
  scope: Scope;
  /** Transcript shown next to the chat; its citations seek instead of navigating. */
  currentId?: string;
  /** Titles for citations into other transcripts. */
  titles?: Record<string, string>;
  /** Offer "read the whole transcript" (single-transcript scope). */
  allowReadAll?: boolean;
  suggestions?: string[];
  /** Sent as soon as the chat mounts (from the search dialog). */
  initial?: string;
}>();
const emit = defineEmits<{ seek: [time: number]; open: [id: string, time: number] }>();

const ai = useAi();
const threads = ref<AiThread[]>([]);
const thread = ref<AiThread | null>(null);
const messages = ref<AiMessage[]>([]);
const question = ref("");
const readAll = ref(false);
const error = ref<string | null>(null);
const scroller = ref<HTMLElement | null>(null);

const runningKey = computed(() => (thread.value ? `ask:${thread.value.id}` : null));
const busy = computed(() => !!runningKey.value && ai.isRunning(runningKey.value));
const streaming = computed(() => (thread.value ? ai.streams[thread.value.id] : undefined));
const status = computed(() => (runningKey.value ? ai.running[runningKey.value]?.message : undefined));

const defaultSuggestions = [
  "What was decided?",
  "List the action items and who owns them",
  "What problems or risks were raised?",
  "Summarise the discussion about the app",
];

async function loadThreads() {
  threads.value = await api.aiListThreads(props.scope);
}

async function openThread(t: AiThread | null) {
  thread.value = t;
  error.value = null;
  messages.value = t ? ((await api.aiGetThread(t.id))?.messages ?? []) : [];
  scrollDown();
}

async function send(text = question.value) {
  const q = text.trim();
  if (!q || busy.value) return;
  question.value = "";
  error.value = null;
  // Show the question at once; the stored copy replaces it when the answer lands.
  messages.value.push({
    id: -Date.now(), thread_id: thread.value?.id ?? "", role: "user", content: q, citations: [], model: null,
    input_tokens: 0, cached_tokens: 0, output_tokens: 0, cost_usd: 0, created_at: new Date().toISOString(), from_memo: false,
  });
  scrollDown();
  try {
    const id = await ai.ask({ thread_id: thread.value?.id ?? null, scope: props.scope, question: q, read_all: readAll.value });
    if (!thread.value) {
      await loadThreads();
      thread.value = threads.value.find((t) => t.id === id) ?? { id, title: q, scope: props.scope, created_at: "", updated_at: "" };
    }
  } catch (e) {
    error.value = String(e);
    messages.value.pop();
  }
}

const stop = ai.onAnswer(async (e) => {
  if (!thread.value || e.thread_id !== thread.value.id) return;
  if (e.error) {
    error.value = e.error;
    if (e.error !== "Cancelled") toast.error("Couldn't answer", { description: e.error });
  }
  messages.value = (await api.aiGetThread(thread.value.id))?.messages ?? messages.value;
  await loadThreads();
  scrollDown();
});
onBeforeUnmount(stop);

async function remove(t: AiThread) {
  await api.aiDeleteThread(t.id);
  if (thread.value?.id === t.id) await openThread(null);
  await loadThreads();
}

function onClick(e: MouseEvent) {
  const c = citationFromClick(e);
  if (!c) return;
  if (c.id === props.currentId) emit("seek", c.time);
  else emit("open", c.id, c.time);
}

function onKey(e: KeyboardEvent) {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    void send();
  }
}

function scrollDown() {
  void nextTick(() => scroller.value?.scrollTo({ top: scroller.value.scrollHeight }));
}

watch(streaming, scrollDown);
watch(() => props.scope, async () => { await loadThreads(); await openThread(null); }, { deep: true });
onMounted(async () => {
  await loadThreads();
  if (props.initial) void send(props.initial);
});

function cost(m: AiMessage) {
  if (m.from_memo) return "answered from an earlier identical question · no tokens";
  const cached = m.cached_tokens ? ` (${formatTokens(m.cached_tokens)} cached)` : "";
  return `${m.model ?? ""} · ${formatTokens(m.input_tokens)} in${cached} / ${formatTokens(m.output_tokens)} out · ${formatUsd(m.cost_usd)}`;
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div class="flex items-center gap-1.5 border-b px-3 py-2">
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="ghost" size="sm" class="min-w-0 max-w-[60%] justify-start">
            <span class="truncate">{{ thread?.title ?? "New conversation" }}</span>
            <ChevronDown class="size-3.5 shrink-0 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" class="w-80">
          <DropdownMenuItem @click="openThread(null)"><MessageSquarePlus /> New conversation</DropdownMenuItem>
          <DropdownMenuSeparator v-if="threads.length" />
          <DropdownMenuItem v-for="t in threads" :key="t.id" class="group items-start" @click="openThread(t)">
            <div class="grid min-w-0 flex-1">
              <span class="truncate">{{ t.title }}</span>
              <span class="text-muted-foreground text-xs">{{ formatDate(t.updated_at) }}</span>
            </div>
            <button class="text-muted-foreground hover:text-destructive opacity-0 group-hover:opacity-100" title="Delete conversation" @click.stop="remove(t)">
              <Trash2 class="size-3.5" />
            </button>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <span class="flex-1" />
      <Button
        v-if="allowReadAll"
        variant="ghost"
        size="sm"
        :class="readAll ? 'text-primary' : 'text-muted-foreground'"
        title="Put the whole transcript in front of the model instead of the most relevant parts. Slower and costlier per question; best for a burst of detailed questions."
        @click="readAll = !readAll"
      ><BookOpen /> Read all</Button>
    </div>

    <div ref="scroller" class="min-h-0 flex-1 overflow-y-auto px-3 py-3" @click="onClick">
      <div v-if="!messages.length && !streaming" class="text-muted-foreground flex h-full flex-col justify-end gap-2 text-sm">
        <p>Ask anything about {{ currentId ? "this meeting" : "these meetings" }}. Answers cite the moment they come from; click a time to jump there.</p>
        <div class="flex flex-wrap gap-1.5">
          <button
            v-for="s in suggestions ?? defaultSuggestions"
            :key="s"
            class="hover:bg-accent rounded-full border px-2.5 py-1 text-left text-xs"
            @click="send(s)"
          >{{ s }}</button>
        </div>
      </div>
      <div v-for="m in messages" :key="m.id" class="mb-3">
        <div v-if="m.role === 'user'" class="bg-primary/10 ml-8 rounded-2xl rounded-br-sm px-3 py-2 text-sm whitespace-pre-wrap">{{ m.content }}</div>
        <div v-else class="mr-2">
          <div class="prose-ai text-sm" v-html="renderMarkdown(m.content, currentId, titles)" />
          <p class="text-muted-foreground mt-1 text-[11px]">{{ cost(m) }}</p>
        </div>
      </div>
      <div v-if="busy" class="mr-2">
        <div v-if="streaming" class="prose-ai text-sm" v-html="renderMarkdown(streaming, currentId, titles)" />
        <p class="text-muted-foreground mt-1 flex items-center gap-1.5 text-xs">
          <Loader2 class="size-3 animate-spin" /> {{ status ?? "Thinking" }}
        </p>
      </div>
      <p v-if="error && error !== 'Cancelled'" class="text-destructive mt-2 text-xs break-words">{{ error }}</p>
    </div>

    <div class="border-t p-2">
      <div class="flex items-end gap-1.5">
        <Textarea
          v-model="question"
          :placeholder="currentId ? 'Ask about this meeting…' : 'Ask about these meetings…'"
          class="max-h-32 min-h-9 resize-none"
          rows="1"
          @keydown="onKey"
        />
        <Button v-if="busy && runningKey" size="icon" variant="outline" title="Stop" @click="ai.cancel(runningKey)"><Square /></Button>
        <Button v-else size="icon" :disabled="!question.trim()" title="Send (Enter)" @click="send()"><Send /></Button>
      </div>
    </div>
  </div>
</template>
