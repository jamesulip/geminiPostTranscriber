import { createRouter, createWebHashHistory } from "vue-router";
import LibraryView from "./views/LibraryView.vue";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "library", component: LibraryView },
    { path: "/t/:id", name: "transcript", component: () => import("./views/TranscriptView.vue") },
    { path: "/usage", name: "usage", component: () => import("./views/UsageView.vue") },
    { path: "/settings", name: "settings", component: () => import("./views/SettingsView.vue") },
  ],
});
