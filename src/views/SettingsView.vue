<script setup lang="ts">
import { onMounted, reactive, ref, watchEffect } from "vue";
import { Eye, EyeOff, Plus, Trash2 } from "@lucide/vue";
import { toast } from "vue-sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { api, diarizes, type SilenceMethod, type Settings } from "@/lib/api";
import { useSettings } from "@/stores/settings";

const store = useSettings();
const form = ref<Settings | null>(null);
const reveal = ref<Record<string, boolean>>({});
const BUILTIN = ["groq", "openai", "gemini"];
// Built-in price estimate per provider, shown as the placeholder of the price field.
const estimates = reactive<Record<string, { per_minute: number; tokens: [number, number] | null }>>({});
watchEffect(() => {
  for (const p of form.value?.providers ?? []) {
    const model = p.model;
    void api.knownPrice(model).then((v) => (estimates[p.id] = v));
  }
});

type Provider = Settings["providers"][number];
type PriceKey = "price_per_minute" | "price_input_per_mtok" | "price_output_per_mtok";

/** Optional numeric field: empty input = null (use the built-in estimate). */
function setPrice(p: Provider, key: PriceKey, v: string | number) {
  const n = typeof v === "number" ? v : parseFloat(v);
  p[key] = v === "" || Number.isNaN(n) ? null : n;
}

onMounted(async () => {
  form.value = JSON.parse(JSON.stringify(await store.load()));
});

function addProvider() {
  form.value!.providers.push({
    id: `custom-${Date.now()}`,
    name: "Custom (OpenAI-compatible)",
    kind: "openai",
    base_url: "http://localhost:8000/v1",
    api_key: "",
    model: "whisper-1",
    price_per_minute: null,
    price_input_per_mtok: null,
    price_output_per_mtok: null,
  });
}

function removeProvider(id: string) {
  const f = form.value!;
  f.providers = f.providers.filter((p) => p.id !== id);
  if (f.default_provider === id) f.default_provider = f.providers[0]?.id ?? "";
  if (f.diarize_provider === id) f.diarize_provider = f.providers.find((p) => diarizes(p))?.id ?? f.default_provider;
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
                <span class="text-muted-foreground text-xs">{{ p.kind === "gemini" ? "Gemini API" : "OpenAI-compatible" }}</span>
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
                <div class="grid gap-1.5"><Label>Default model</Label><Input v-model="p.model" /></div>
                <div class="grid gap-1.5">
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
                  <p class="text-muted-foreground text-xs">Used instead of the per-minute price when the provider reports tokens.</p>
                </div>
              </div>
            </div>
            <Button variant="outline" class="w-fit" @click="addProvider"><Plus /> Add OpenAI-compatible provider</Button>
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
                  <SelectItem v-for="p in form.providers" :key="p.id" :value="p.id">{{ p.name }}</SelectItem>
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
              <p class="text-muted-foreground text-xs">Diarized requests are limited to 30 min. Longer chunks keep speaker labels more consistent.</p>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  </div>
</template>
