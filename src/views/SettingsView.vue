<script setup lang="ts">
import { computed, onMounted, reactive, ref, watchEffect } from "vue";
import { Eye, EyeOff, Loader2, Plus, Sparkles, Trash2 } from "@lucide/vue";
import { Textarea } from "@/components/ui/textarea";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { api, defaultChatModel, diarizes, transcribes, PROVIDER_KIND_LABEL, type ProviderKind, type SilenceMethod, type Settings } from "@/lib/api";
import { useAi } from "@/stores/ai";
import { useSettings } from "@/stores/settings";

const store = useSettings();
const form = ref<Settings | null>(null);
const reveal = ref<Record<string, boolean>>({});
const BUILTIN = ["groq", "openai", "gemini", "anthropic"];
const ai = useAi();
const testing = ref(false);
const testResult = ref<string | null>(null);

const aiProvider = computed(() => form.value?.providers.find((p) => p.id === form.value!.ai_provider));
const chatPlaceholder = computed(() => (aiProvider.value ? defaultChatModel(aiProvider.value.kind) : ""));

async function testAi() {
  const f = form.value!;
  testing.value = true;
  testResult.value = null;
  try {
    // Test what's on screen: save first so the backend sees the key and model being tried.
    await store.save(f);
    testResult.value = await api.aiTestConnection(f.ai_provider, f.ai_model || chatPlaceholder.value);
  } catch (e) {
    testResult.value = `Failed: ${e}`;
  } finally {
    testing.value = false;
  }
}

function addTemplate() {
  form.value!.ai_templates.push({ id: `custom-${Date.now()}`, name: "", description: "", prompt: "" });
}

async function indexLibrary() {
  try {
    await store.save(form.value!);
    await api.aiEmbedLibrary();
    toast("Indexing the library for meaning search", { description: "Runs in the background; progress shows in the sidebar." });
  } catch (e) {
    toast.error("Couldn't start indexing", { description: String(e) });
  }
}
// Built-in price estimate per provider, shown as the placeholder of the price field.
const estimates = reactive<Record<string, { per_minute: number; tokens: [number, number] | null; text: { input: number; output: number; cached_input: number } | null }>>({});
watchEffect(() => {
  for (const p of form.value?.providers ?? []) {
    const model = p.model;
    void api.knownPrice(model).then((v) => (estimates[p.id] = v));
  }
});

type Provider = Settings["providers"][number];
type PriceKey = "price_per_minute" | "price_input_per_mtok" | "price_output_per_mtok" | "price_cached_input_per_mtok";

/** Optional numeric field: empty input = null (use the built-in estimate). */
function setPrice(p: Provider, key: PriceKey, v: string | number) {
  const n = typeof v === "number" ? v : parseFloat(v);
  p[key] = v === "" || Number.isNaN(n) ? null : n;
}

onMounted(async () => {
  form.value = JSON.parse(JSON.stringify(await store.load()));
});

function addProvider(kind: ProviderKind = "openai") {
  form.value!.providers.push({
    id: `${kind === "anthropic" ? "anthropic" : "custom"}-${Date.now()}`,
    name: kind === "anthropic" ? "Anthropic" : "Custom (OpenAI-compatible)",
    kind,
    base_url: kind === "anthropic" ? "https://api.anthropic.com" : "http://localhost:8000/v1",
    api_key: "",
    model: kind === "anthropic" ? "claude-opus-5-5" : "whisper-1",
    price_per_minute: null,
    price_input_per_mtok: null,
    price_output_per_mtok: null,
    price_cached_input_per_mtok: null,
  });
}

function removeProvider(id: string) {
  const f = form.value!;
  f.providers = f.providers.filter((p) => p.id !== id);
  if (f.default_provider === id) f.default_provider = f.providers[0]?.id ?? "";
  if (f.diarize_provider === id) f.diarize_provider = f.providers.find((p) => diarizes(p))?.id ?? f.default_provider;
  if (f.ai_provider === id) f.ai_provider = f.providers[0]?.id ?? "";
  if (f.ai_embedding_provider === id) f.ai_embedding_provider = f.providers.find((p) => p.kind !== "anthropic")?.id ?? "";
}

async function save() {
  try {
    await store.save(form.value!);
    toast.success("Settings saved");
  } catch (e) {
    toast.error("Couldn't save settings", { description: String(e) });
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex items-center border-b px-6 py-4" data-tauri-drag-region>
      <h1 class="text-lg font-semibold">Settings</h1>
      <Button class="ml-auto" :disabled="!form" @click="save">Save</Button>
    </header>
    <div v-if="form" class="min-h-0 flex-1 overflow-y-auto p-6">
      <div class="mx-auto grid max-w-3xl gap-6">
        <Card>
          <CardHeader>
            <CardTitle>Providers</CardTitle>
            <CardDescription>
              API keys are stored locally in the app's data folder (plain text). Groq and OpenAI use the OpenAI
              <code>/audio/transcriptions</code> API; Gemini <code>*-transcribe</code> models provide speaker diarization.
            </CardDescription>
          </CardHeader>
          <CardContent class="grid gap-4">
            <div v-for="p in form.providers" :key="p.id" class="grid gap-3 rounded-lg border p-4">
              <div class="flex items-center gap-2">
                <Input v-model="p.name" class="h-8 max-w-56 font-medium" />
                <span class="text-muted-foreground text-xs">{{ PROVIDER_KIND_LABEL[p.kind] }}<template v-if="p.kind === 'anthropic'"> · assistant only (no audio)</template></span>
                <Button v-if="!BUILTIN.includes(p.id)" variant="ghost" size="icon" class="ml-auto" @click="removeProvider(p.id)">
                  <Trash2 />
                </Button>
              </div>
              <div class="grid grid-cols-2 gap-3">
                <div class="col-span-2 grid gap-1.5">
                  <Label>API key</Label>
                  <div class="flex gap-2">
                    <Input v-model="p.api_key" :type="reveal[p.id] ? 'text' : 'password'" placeholder="Paste API key" autocomplete="off" />
                    <Button variant="outline" size="icon" @click="reveal[p.id] = !reveal[p.id]">
                      <EyeOff v-if="reveal[p.id]" /><Eye v-else />
                    </Button>
                  </div>
                </div>
                <div class="grid gap-1.5"><Label>Base URL</Label><Input v-model="p.base_url" /></div>
                <div class="grid gap-1.5"><Label>{{ p.kind === "anthropic" ? "Default chat model" : "Default transcription model" }}</Label><Input v-model="p.model" /></div>
                <div v-if="transcribes(p)" class="grid gap-1.5">
                  <Label>Price per audio minute (USD)</Label>
                  <Input
                    :model-value="p.price_per_minute ?? ''"
                    type="number"
                    min="0"
                    step="0.0001"
                    :placeholder="estimates[p.id] ? `${estimates[p.id].per_minute.toFixed(5)} (built-in)` : ''"
                    @update:model-value="(v) => setPrice(p, 'price_per_minute', v)"
                  />
                </div>
                <div class="grid gap-1.5">
                  <Label>Token price per 1M, in / out (USD)</Label>
                  <div class="flex gap-2">
                    <Input
                      :model-value="p.price_input_per_mtok ?? ''"
                      type="number"
                      min="0"
                      step="0.01"
                      :placeholder="estimates[p.id]?.tokens ? `${estimates[p.id].tokens![0]} (built-in)` : 'in'"
                      @update:model-value="(v) => setPrice(p, 'price_input_per_mtok', v)"
                    />
                    <Input
                      :model-value="p.price_output_per_mtok ?? ''"
                      type="number"
                      min="0"
                      step="0.01"
                      :placeholder="estimates[p.id]?.tokens ? `${estimates[p.id].tokens![1]} (built-in)` : 'out'"
                      @update:model-value="(v) => setPrice(p, 'price_output_per_mtok', v)"
                    />
                  </div>
                  <p class="text-muted-foreground text-xs">Used instead of the per-minute price when the provider reports tokens; also the assistant's text rates for this provider's chat model.</p>
                </div>
                <div class="grid gap-1.5">
                  <Label>Cached prompt tokens per 1M (USD)</Label>
                  <Input
                    :model-value="p.price_cached_input_per_mtok ?? ''"
                    type="number"
                    min="0"
                    step="0.01"
                    :placeholder="estimates[p.id]?.text ? `${estimates[p.id].text!.cached_input} (built-in)` : '25% of input'"
                    @update:model-value="(v) => setPrice(p, 'price_cached_input_per_mtok', v)"
                  />
                  <p class="text-muted-foreground text-xs">Assistant only: what the provider charges for prompt tokens served from its cache.</p>
                </div>
              </div>
            </div>
            <div class="flex flex-wrap gap-2">
              <Button variant="outline" class="w-fit" @click="addProvider()"><Plus /> Add OpenAI-compatible provider</Button>
              <Button variant="outline" class="w-fit" @click="addProvider('anthropic')"><Plus /> Add Anthropic provider</Button>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle class="flex items-center gap-2"><Sparkles class="text-primary size-4" /> AI assistant</CardTitle>
            <CardDescription>
              Summaries, questions with citations, and meaning search over your transcripts. Each transcript is summarised once
              and indexed; questions then send only the relevant parts, so a long meeting isn't re-read every time.
            </CardDescription>
          </CardHeader>
          <CardContent class="grid grid-cols-2 gap-4">
            <div class="grid gap-1.5">
              <Label>Provider for questions and summaries</Label>
              <Select v-model="form.ai_provider">
                <SelectTrigger class="w-full"><SelectValue placeholder="Pick a provider" /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="p in form.providers" :key="p.id" :value="p.id">{{ p.name }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div class="grid gap-1.5">
              <Label>Chat model</Label>
              <Input v-model="form.ai_model" :placeholder="chatPlaceholder" />
              <p class="text-muted-foreground text-xs">Gemini: gemini-3.8-flash ($0.75 / $3.75 per 1M, cached $0.075) has context caching. Anthropic: claude-opus-5-5 (claude-sonnet-5-5 is cheaper). Groq/OpenAI: any chat model.</p>
            </div>
            <div class="col-span-2 flex items-center gap-3">
              <Button variant="outline" size="sm" :disabled="testing || !form.ai_provider" @click="testAi">
                <Loader2 v-if="testing" class="animate-spin" /> Test connection
              </Button>
              <span v-if="testResult" class="text-xs" :class="testResult.startsWith('Failed') ? 'text-destructive' : 'text-muted-foreground'">{{ testResult }}</span>
            </div>
            <div class="grid gap-1.5">
              <Label>Provider for meaning search (embeddings)</Label>
              <Select v-model="form.ai_embedding_provider">
                <SelectTrigger class="w-full"><SelectValue placeholder="Pick a provider" /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="p in form.providers.filter((x) => x.kind !== 'anthropic')" :key="p.id" :value="p.id">{{ p.name }}</SelectItem>
                </SelectContent>
              </Select>
              <p class="text-muted-foreground text-xs">Anthropic has no embeddings; Gemini or OpenAI work. Must handle Filipino and English.</p>
            </div>
            <div class="grid gap-1.5">
              <Label>Embedding model</Label>
              <Input v-model="form.ai_embedding_model" placeholder="gemini-embedding-2" />
              <p class="text-muted-foreground text-xs">gemini-embedding-2 (multilingual, $0.20 per 1M tokens) or gemini-embedding-001; OpenAI: text-embedding-3-small. Changing it re-indexes everything on next use.</p>
            </div>
            <div class="col-span-2 flex items-start justify-between gap-4 rounded-lg border p-3">
              <div class="grid gap-0.5">
                <Label for="ai-auto">Summarise each transcript when it finishes</Label>
                <p class="text-muted-foreground text-xs">
                  Names the meeting, builds decisions/action items/topics/glossary and indexes it for search. Costs cents per meeting
                  and is only redone when the transcript changes.
                </p>
              </div>
              <Switch id="ai-auto" v-model="form.ai_auto_digest" />
            </div>
            <div class="col-span-2 flex items-start justify-between gap-4 rounded-lg border p-3">
              <div class="grid gap-0.5">
                <Label for="ai-cache">Keep the whole transcript cached at the provider while asking</Label>
                <p class="text-muted-foreground text-xs">
                  Gemini only (Anthropic caches automatically). With “Read all” on, the transcript is uploaded once and later
                  questions pay the cached-input rate; Google charges a small storage fee per hour it stays cached.
                </p>
                <div class="mt-2 flex items-center gap-2">
                  <Label class="text-xs">Keep for (minutes)</Label>
                  <Input v-model.number="form.ai_context_cache_ttl_minutes" type="number" min="5" max="1440" class="h-7 w-24" />
                </div>
              </div>
              <Switch id="ai-cache" v-model="form.ai_context_cache" />
            </div>
            <div class="col-span-2 grid gap-3 rounded-lg border p-3">
              <div>
                <Label>Custom document templates</Label>
                <p class="text-muted-foreground text-xs">
                  Besides the built-in minutes, action-item email, status update, brief, decision log, user stories, task list and Q&A.
                  Describe the document you want (sections, tone, length); the app adds the meeting material and the citation rules.
                </p>
              </div>
              <div v-for="(tpl, i) in form.ai_templates" :key="tpl.id" class="grid gap-2 rounded-md border p-3">
                <div class="flex gap-2">
                  <Input v-model="tpl.name" placeholder="Name, e.g. Client recap" class="h-8 max-w-64" />
                  <Input v-model="tpl.description" placeholder="One-line description" class="h-8" />
                  <Button variant="ghost" size="icon" class="size-8 shrink-0" @click="form.ai_templates.splice(i, 1)"><Trash2 /></Button>
                </div>
                <Textarea v-model="tpl.prompt" rows="4" placeholder="Write a recap for the client with: ## What we agreed, ## What we need from you, ## Timeline…" class="text-sm" />
              </div>
              <Button variant="outline" size="sm" class="w-fit" @click="addTemplate"><Plus /> Add template</Button>
            </div>
            <div class="col-span-2 flex items-center gap-3">
              <Button variant="outline" size="sm" :disabled="ai.isRunning('embed:library')" @click="indexLibrary">
                <Loader2 v-if="ai.isRunning('embed:library')" class="animate-spin" /> Index library for meaning search
              </Button>
              <span class="text-muted-foreground text-xs">
                {{ ai.running["embed:library"]?.message ?? "Embeds transcripts that aren't indexed yet (summaries do this automatically)." }}
              </span>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Defaults</CardTitle>
            <CardDescription>Used to pre-fill the new transcription dialog.</CardDescription>
          </CardHeader>
          <CardContent class="grid grid-cols-2 gap-4">
            <div class="grid gap-1.5">
              <Label>Provider — text only</Label>
              <Select v-model="form.default_provider">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="p in form.providers.filter((x) => transcribes(x))" :key="p.id" :value="p.id">{{ p.name }}</SelectItem>
                </SelectContent>
              </Select>
              <p class="text-muted-foreground text-xs">Default. Pick the cheapest — Groq turbo is ~$0.04 per hour sent.</p>
            </div>
            <div class="grid gap-1.5">
              <Label>Provider — with speaker labels</Label>
              <Select v-model="form.diarize_provider">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="p in form.providers.filter((x) => diarizes(x))" :key="p.id" :value="p.id">{{ p.name }}</SelectItem>
                </SelectContent>
              </Select>
              <p class="text-muted-foreground text-xs">Used when “Speaker labels” is switched on.</p>
            </div>
            <div class="grid gap-1.5"><Label>Language</Label><Input v-model="form.language" placeholder="Auto-detect" /></div>
            <div class="grid gap-1.5">
              <Label>Overnight (batch) model</Label>
              <Input v-model="form.batch_model" placeholder="gemini-3.8-flash" />
              <p class="text-muted-foreground text-xs">Gemini model used via the Batch API (~50% cheaper, results within 24 h).</p>
            </div>
            <div class="grid gap-1.5">
              <Label>Chunk length — overnight (min)</Label>
              <Input v-model.number="form.batch_chunk_minutes" type="number" min="1" max="30" />
            </div>
            <div class="col-span-2 flex items-start justify-between gap-4 rounded-lg border p-3">
              <div class="grid gap-0.5">
                <Label for="batch-retry">Re-do failed overnight parts automatically</Label>
                <p class="text-muted-foreground text-xs">
                  When Google blocks or skips part of a batch, transcribe just that part right away with
                  Gemini 3.5 Flash, or 3.5 Transcribe if that's blocked too. Off: the parts are flagged for “Fix problems”.
                </p>
              </div>
              <Switch id="batch-retry" v-model="form.batch_retry_failed" />
            </div>
            <div />
            <div class="grid gap-1.5">
              <Label>Speed — text only (1.0–2.0)</Label>
              <Input v-model.number="form.speed" type="number" min="1" max="2" step="0.05" />
            </div>
            <div class="grid gap-1.5">
              <Label>Speed — with speaker labels</Label>
              <Input v-model.number="form.diarize_speed" type="number" min="1" max="2" step="0.05" />
            </div>
            <div class="flex items-end justify-between gap-3 pb-2">
              <Label for="def-rm">Remove silence</Label>
              <Switch id="def-rm" v-model="form.remove_silence" />
            </div>
            <div class="grid gap-1.5">
              <Label>Detect</Label>
              <Select :model-value="form.silence_method" @update:model-value="(v) => (form!.silence_method = v as SilenceMethod)">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="speech">Non-speech (AI, recommended)</SelectItem>
                  <SelectItem value="volume">Quiet only (volume)</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div class="grid gap-1.5">
              <Label>Speech threshold (0.2–0.8)</Label>
              <Input v-model.number="form.vad_threshold" type="number" min="0.2" max="0.8" step="0.05" />
            </div>
            <div class="grid gap-1.5">
              <Label>Silence threshold — volume (dB)</Label>
              <Input v-model.number="form.silence_threshold_db" type="number" min="-60" max="-20" step="1" />
            </div>
            <div class="grid gap-1.5">
              <Label>Minimum silence (s)</Label>
              <Input v-model.number="form.silence_min_duration" type="number" min="0.2" max="5" step="0.1" />
            </div>
            <div class="grid gap-1.5">
              <Label>Chunk length — Whisper (min)</Label>
              <Input v-model.number="form.chunk_minutes" type="number" min="1" max="30" />
            </div>
            <div class="grid gap-1.5">
              <Label>Chunk length — Gemini (min)</Label>
              <Input v-model.number="form.gemini_chunk_minutes" type="number" min="1" max="30" />
              <p class="text-muted-foreground text-xs">Shorter chunks are more reliable (long ones can skip or repeat speech); longer ones keep speaker labels more consistent. 10 min recommended.</p>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  </div>
</template>
