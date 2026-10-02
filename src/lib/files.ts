import { open } from "@tauri-apps/plugin-dialog";
import { AUDIO_EXTENSIONS } from "./format";

export async function pickAudioFiles(): Promise<string[]> {
  const res = await open({
    multiple: true,
    directory: false,
    filters: [{ name: "Audio / video", extensions: AUDIO_EXTENSIONS }],
  });
  if (!res) return [];
  return Array.isArray(res) ? res : [res];
}
