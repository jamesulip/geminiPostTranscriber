<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { ArrowLeft, ClipboardCopy, Download, FileText, Loader2, RefreshCw, AlertTriangle } from "@lucide/vue";
import { save } from "@tauri-apps/plugin-dialog";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import { api, type AiTemplate, type Digest, type Scope } from "@/lib/api";
import { formatDate } from "@/lib/format";
import { citationFromClick, renderMarkdown, stripCitations } from "@/lib/markdown";
import { useAi } from "@/stores/ai";

const props = defineProps<{
  scope: Scope;
  /** Transcript shown beside the documents: its citations seek, and its documents are stored. */
  currentId?: string;
  titles?: Record<string, string>;
}>();
const emit = defineEmits<{ seek: [time: number]; open: [id: string, time: number] }>();

const ai = useAi();
const templates = ref<AiTemplate[]>([]);
/** Stored documents of the current transcript, by template id. */
const stored = ref<Record<string, Digest>>({});
const openId = ref<string | null>(null);
const text = ref<string | null>(null);
const generating = ref<string | null>(null);

const current = computed(() => templates.value.find((t) => t.id === openId.value));
const storedDoc = computed(() => (openId.value ? stored.value[openId.value] : undefined));

async function load() {
  templates.value = await api.aiTemplates();
  if (props.currentId) {
    const ds = await api.aiGetDigests(props.currentId);
    stored.value = Object.fromEntries(ds.filter((d) => d.kind.startsWith("doc:")).map((d) => [d.kind.slice(4), d]));
  }
}
onMounted(load);
watch(() => props.currentId && ai.digestVersion[props.currentId], load);
watch(() => props.scope, () => { openId.value = null; text.value = null; void load(); }, { deep: true });

function open(t: AiTemplate) {
  openId.value = t.id;
  const s = stored.value[t.id];
  text.value = s ? s.content : null;
  if (!s) void generate(t.id);
}

async function generate(id: string) {
  if (generating.value) return;
  generating.value = id;
  try {
    text.value = await api.aiDocument(props.scope, id);
    await load();
  } catch (e) {
    toast.error("Couldn't write the document", { description: String(e) });
    if (!text.value) openId.value = null;
  } finally {
    generating.value = null;
  }
}

async function copy() {
  if (!text.value) return;
  await navigator.clipboard.writeText(text.value);
  toast.success("Copied as markdown");
}

async function download() {
  if (!text.value || !current.value) return;
  const path = await save({ defaultPath: `${current.value.name}.md`, filters: [{ name: "Markdown", extensions: ["md"] }] });
  if (!path) return;
  await api.saveText(path, text.value);
  toast.success("Saved");
}

function onClick(e: MouseEvent) {
  const c = citationFromClick(e);
  if (!c) return;
  if (c.id === props.currentId) emit("seek", c.time);
  else emit("open", c.id, c.time);
}

/** Plain text for the copy button when citations would be noise (e.g. an email). */
async function copyPlain() {
  if (!text.value) return;
  await navigator.clipboard.writeText(stripCitations(text.value));
  toast.success("Copied without citations");
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <template v-if="!openId">
      <p class="text-muted-foreground px-4 pt-3 pb-2 text-xs">
        Written from the stored summaries with citations; a document is kept until the transcript changes.
      </p>
      <div class="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        <button
          v-for="t in templates"
          :key="t.id"
          class="hover:bg-accent flex w-full items-start gap-3 rounded-lg px-2 py-2 text-left"
          @click="open(t)"
        >
          <FileText class="text-primary mt-0.5 size-4 shrink-0" />
          <span class="min-w-0 flex-1">
            <span class="flex items-center gap-2 text-sm font-medium">
              {{ t.name }}
              <span v-if="stored[t.id]" class="bg-muted text-muted-foreground rounded px-1.5 text-[10px] font-normal">
                {{ stored[t.id].stale ? "saved · outdated" : "saved" }}
              </span>
              <span v-if="!t.builtin" class="bg-primary/10 text-primary rounded px-1.5 text-[10px] font-normal">custom</span>
            </span>
            <span class="text-muted-foreground block text-xs">{{ t.description }}</span>
          </span>
        </button>
        <p class="text-muted-foreground px-2 pt-2 text-[11px]">Add your own templates under Settings → AI assistant.</p>
      </div>
    </template>
    <template v-else>
      <div class="flex items-center gap-1 border-b px-2 py-1.5">
        <Button variant="ghost" size="icon" class="size-7" title="All documents" @click="(openId = null), (text = null)"><ArrowLeft /></Button>
        <span class="min-w-0 flex-1 truncate text-sm font-medium">{{ current?.name }}</span>
        <Button variant="ghost" size="icon" class="size-7" title="Copy as markdown" :disabled="!text" @click="copy"><ClipboardCopy /></Button>
        <Button variant="ghost" size="icon" class="size-7" title="Save as .md" :disabled="!text" @click="download"><Download /></Button>
        <Button variant="ghost" size="icon" class="size-7" title="Write again" :disabled="!!generating" @click="generate(openId!)"><RefreshCw /></Button>
      </div>
      <div class="min-h-0 flex-1 overflow-y-auto px-4 py-3 text-sm" @click="onClick">
        <div v-if="generating" class="text-muted-foreground flex items-center gap-2 py-6 text-sm">
          <Loader2 class="size-4 animate-spin" /> Writing {{ current?.name.toLowerCase() }}… about half a minute.
        </div>
        <template v-else-if="text">
          <div v-if="storedDoc?.stale" class="mb-3 flex items-center gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs dark:border-amber-900 dark:bg-amber-950/40">
            <AlertTriangle class="size-3.5 shrink-0 text-amber-600" /> The transcript changed since this was written.
            <Button size="sm" variant="outline" class="ml-auto h-6" @click="generate(openId!)">Rewrite</Button>
          </div>
          <div class="prose-ai" v-html="renderMarkdown(text, currentId, titles)" />
          <p class="text-muted-foreground mt-4 flex items-center gap-2 text-[11px]">
            <span v-if="storedDoc">{{ storedDoc.model }} · {{ formatDate(storedDoc.created_at) }}</span>
            <button class="hover:text-foreground underline" @click="copyPlain">copy without citations</button>
          </p>
        </template>
      </div>
    </template>
  </div>
</template>
