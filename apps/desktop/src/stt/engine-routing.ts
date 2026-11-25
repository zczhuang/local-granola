import {
  commands as transcriptionCommands,
  type SttRouteDecision,
} from "@anlg/plugin-transcription";

import { isRealtimeLocalModel } from "./capabilities";

// LOCAL-GRANOLA: language-based engine routing for the live path.
//
// Parakeet gives fast live text but cannot do Mandarin or mixed-language meetings. Once enough
// speech has been transcribed, the Rust `stt-routing` router (via `decide_stt_route`) is asked
// which engine the meeting needs. Switching the live engine mid-meeting would drop audio and
// duplicate words, so the smaller safe change is used instead: when the verdict is Whisper (and the
// user's override is `auto`) the session is marked, and the post-meeting re-pass runs as if
// `stt_whisper_repass` were `always`, with Whisper pinned to the detected language (or
// auto-detecting per chunk for mixed meetings).

export type SttEngineOverride = "auto" | "parakeet" | "whisper";

export const DEFAULT_STT_ENGINE_OVERRIDE: SttEngineOverride = "auto";

// Same as the router's `min_speech_ms`: it refuses to decide earlier unless confidence is very high.
export const ROUTING_MIN_SPEECH_MS = 30_000;

export function normalizeSttEngineOverride(value: unknown): SttEngineOverride {
  return value === "auto" || value === "parakeet" || value === "whisper"
    ? value
    : DEFAULT_STT_ENGINE_OVERRIDE;
}

export type ForcedRepass = {
  // Languages to hand to the Whisper batch run; empty lets it auto-detect.
  languages: string[];
};

// Pure: what a router verdict means for this session. Only an `auto` override on a fast local
// live engine can force a re-pass; explicit overrides are the user's call, and a session that is
// already on Whisper or a cloud engine has nothing to switch.
export function forcedRepassFor(
  decision: SttRouteDecision,
  override: unknown,
  liveEngine: string | null | undefined,
): ForcedRepass | null {
  if (normalizeSttEngineOverride(override) !== "auto") return null;
  if (!isRealtimeLocalModel(liveEngine)) return null;
  if (decision.engine !== "whisper") return null;
  return { languages: decision.lang ? [decision.lang] : [] };
}

const forcedRepasses = new Map<string, ForcedRepass>();

export function markForcedRepass(sessionId: string, forced: ForcedRepass) {
  forcedRepasses.set(sessionId, forced);
}

export function getForcedRepass(sessionId: string): ForcedRepass | null {
  return forcedRepasses.get(sessionId) ?? null;
}

export function clearForcedRepass(sessionId: string) {
  forcedRepasses.delete(sessionId);
}

type PersistedDelta = {
  new_words: { text: string; start_ms: number; end_ms: number }[];
};

// Accumulates the finalized live words of one session and, once ~30 s of speech exist, asks the
// router once. Event driven (no timers): it is fed from the live persist callback.
export function createEngineRoutingProbe(options: {
  sessionId: string;
  override: unknown;
  liveEngine: string | null | undefined;
  declaredLanguages: string[];
  minSpeechMs?: number;
  decide?: (input: {
    preference: string;
    declared_languages: string[];
    sample_text: string;
    speech_ms: number;
  }) => Promise<SttRouteDecision>;
}) {
  const minSpeechMs = options.minSpeechMs ?? ROUTING_MIN_SPEECH_MS;
  const decide =
    options.decide ??
    (async (input) => {
      const result = await transcriptionCommands.decideSttRoute(input);
      if (result.status !== "ok") throw new Error(result.error);
      return result.data;
    });
  const active =
    normalizeSttEngineOverride(options.override) === "auto" &&
    isRealtimeLocalModel(options.liveEngine);
  let speechMs = 0;
  let text = "";
  // Speech level at which to (next) ask the router; Infinity while a request is in flight.
  let askAt = minSpeechMs;

  clearForcedRepass(options.sessionId);

  return {
    observe(delta: PersistedDelta) {
      if (!active) return;
      for (const word of delta.new_words) {
        speechMs += Math.max(0, word.end_ms - word.start_ms);
        text += word.text;
      }
      if (speechMs < askAt) return;
      const askedAtSpeechMs = speechMs;
      askAt = Number.POSITIVE_INFINITY;
      void decide({
        preference: "auto",
        declared_languages: options.declaredLanguages,
        sample_text: text,
        speech_ms: speechMs,
      })
        .then((decision) => {
          if (decision.engine === "pending") {
            // The router wanted more evidence; ask again after another stretch of speech.
            askAt = askedAtSpeechMs + minSpeechMs;
            return;
          }
          const forced = forcedRepassFor(
            decision,
            options.override,
            options.liveEngine,
          );
          if (forced) markForcedRepass(options.sessionId, forced);
        })
        .catch((error) => {
          console.error("[stt-routing] route decision failed", error);
          askAt = askedAtSpeechMs + minSpeechMs;
        });
    },
  };
}
