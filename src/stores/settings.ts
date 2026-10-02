import { defineStore } from "pinia";
import { ref } from "vue";
import { api, type Settings } from "@/lib/api";

export const useSettings = defineStore("settings", () => {
  const settings = ref<Settings | null>(null);

  async function load() {
    settings.value = await api.getSettings();
    return settings.value;
  }

  async function save(next: Settings) {
    await api.saveSettings(JSON.parse(JSON.stringify(next)));
    settings.value = JSON.parse(JSON.stringify(next));
  }

  return { settings, load, save };
});
