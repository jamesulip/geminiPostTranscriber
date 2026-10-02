import { diarizes, type JobOptions, type ProviderConfig, type Settings } from "./api";

/** Speed and chunk length suited to a provider. */
export function providerDefaults(s: Settings, p: ProviderConfig | undefined) {
  return {
    speed: diarizes(p) ? s.diarize_speed : s.speed,
    chunk_minutes: p?.kind === "gemini" ? s.gemini_chunk_minutes : s.chunk_minutes,
  };
}

export function defaultOptions(s: Settings, providerId = s.default_provider): JobOptions {
  const p = s.providers.find((x) => x.id === providerId) ?? s.providers[0];
  return {
    provider_id: p.id,
    model: p.model,
    language: s.language || null,
    ...providerDefaults(s, p),
    remove_silence: s.remove_silence,
    silence_method: s.silence_method,
    vad_threshold: s.vad_threshold,
    silence_threshold_db: s.silence_threshold_db,
    silence_min_duration: s.silence_min_duration,
    title: null,
    folder_id: null,
    tag_ids: [],
  };
}
/** Clean up form values before sending to the backend. */
export function normalizeOptions(o: JobOptions): JobOptions {
  return {
    ...o,
    model: o.model?.trim() || null,
    language: o.language?.trim() || null,
    title: o.title?.trim() || null,
    chunk_minutes: Math.min(30, Math.max(1, Number(o.chunk_minutes) || 10)),
    silence_min_duration: Math.max(0.2, Number(o.silence_min_duration) || 0.7),
  };
}
