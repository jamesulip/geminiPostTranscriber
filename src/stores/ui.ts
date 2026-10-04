import { defineStore } from "pinia";
import { ref } from "vue";
import { useLocalStorage } from "@vueuse/core";

export const useUi = defineStore("ui", () => {
  /** Files waiting for the "new transcription" dialog. */
  const pendingFiles = ref<string[]>([]);
  const searchOpen = ref(false);
  /** Library layout, remembered across launches. */
  const libraryView = useLocalStorage<"grid" | "list">("library-view", "grid");
  return { pendingFiles, searchOpen, libraryView };
});
