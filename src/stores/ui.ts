import { defineStore } from "pinia";
import { ref } from "vue";

export const useUi = defineStore("ui", () => {
  /** Files waiting for the "new transcription" dialog. */
  const pendingFiles = ref<string[]>([]);
  const searchOpen = ref(false);
  return { pendingFiles, searchOpen };
});
