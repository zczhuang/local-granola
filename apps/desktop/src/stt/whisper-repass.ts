import { commands as localSttCommands } from "@anlg/plugin-local-stt";
import { commands as transcriptionCommands } from "@anlg/plugin-transcription";

import { isRealtimeLocalModel } from "./capabilities";

import {
  deleteProcessedAudioForRetention,
  registerAudioHold,
  releaseAudioHold,
  type AudioRetentionPolicy,
} from "~/services/audio-retention";

// LOCAL-GRANOLA: after a meeting, optionally re-transcribe the retained audio
// with Whisper large-v3-turbo. Parakeet gives fast live text; Whisper is the
// accurate (and Mandarin-capable) second pass.
export const WHISPER_REPASS_MODEL = "QuantizedLargeTurbo";

export type WhisperRepassSetting = "off" | "auto" | "always";

export const DEFAULT_WHISPER_REPASS: WhisperRepassSetting = "auto";

export function normalizeWhisperRepass(value: unknown): WhisperRepassSetting {
  return value === "off" || value === "auto" || value === "always"
    ? value
    : DEFAULT_WHISPER_REPASS;
}

// auto: only when the live transcript came from a fast on-device engine
// (Parakeet / Apple). always: whatever produced the live text. off: never.
// `forced` (engine routing decided the meeting needs Whisper, see engine-routing.ts) acts as
// `always` unless the user turned the re-pass off.
export function shouldRepass(
  setting: unknown,
  liveEngine: string | null | undefined,
  forced = false,
): boolean {
  switch (normalizeWhisperRepass(setting)) {
    case "off":
      return false;
    case "always":
      return true;
    case "auto":
      return forced || isRealtimeLocalModel(liveEngine);
  }
}

export type WhisperRepassTarget = {
  provider: "anarlog";
  model: typeof WHISPER_REPASS_MODEL;
  baseUrl: string;
  apiKey: "";
};

// Returns null when the Whisper model is missing or its server can't start;
// the caller then keeps the live transcript.
export async function resolveWhisperRepassTarget(): Promise<WhisperRepassTarget | null> {
  try {
    const downloaded =
      await localSttCommands.isModelDownloaded(WHISPER_REPASS_MODEL);
    if (downloaded.status !== "ok" || !downloaded.data) return null;
    const started = await localSttCommands.startServer(WHISPER_REPASS_MODEL);
    if (started.status !== "ok" || !started.data) return null;
    return {
      provider: "anarlog",
      model: WHISPER_REPASS_MODEL,
      baseUrl: started.data,
      apiKey: "",
    };
  } catch {
    return null;
  }
}

// Keeps the session's audio on disk for the duration of the re-pass, then applies the retention
// policy again: the hold may have postponed a delete that was due.
//
// Two holds are needed: the TS one stops TS-initiated deletes, the Rust one stops the plugin's own
// 60 s retention sweep. The Rust hold is best effort (if the command is unavailable the TS hold
// still protects against the common path), but its release is always attempted.
export async function withWhisperRepassHold<T>(
  sessionId: string,
  audioRetention: AudioRetentionPolicy,
  run: () => Promise<T>,
): Promise<T> {
  registerAudioHold(sessionId);
  let rustHeld = false;
  try {
    const held = await transcriptionCommands.acquireSessionAudioHold(sessionId);
    if (held.status === "ok") {
      rustHeld = true;
    } else {
      console.error("[whisper-repass] plugin audio hold refused", held.error);
    }
  } catch (error) {
    console.error("[whisper-repass] failed to hold audio in the plugin", error);
  }
  try {
    return await run();
  } finally {
    releaseAudioHold(sessionId);
    if (rustHeld) {
      try {
        await transcriptionCommands.dropSessionAudioHold(sessionId);
      } catch (error) {
        console.error(
          "[whisper-repass] failed to release the plugin audio hold",
          error,
        );
      }
    }
    await deleteProcessedAudioForRetention(audioRetention, sessionId);
  }
}
