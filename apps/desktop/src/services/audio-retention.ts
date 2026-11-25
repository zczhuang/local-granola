import {
  commands as transcriptionCommands,
  events as transcriptionEvents,
  type SessionAudioRetentionEvent,
} from "@anlg/plugin-transcription";

import type { AudioRetentionPolicy } from "./audio-retention-policy";

import { listenerStore } from "~/store/zustand/listener/instance";

export {
  audioRetentionDays,
  normalizeAudioRetention,
  parseAudioRetentionDays,
  type AudioRetentionPolicy,
} from "./audio-retention-policy";

// Holds keep a session's audio on disk even after processing finishes, e.g.
// while the post-meeting re-pass still needs to read it. Counted so that
// overlapping holders cannot release each other's hold.
const audioHolds = new Map<string, number>();

export function registerAudioHold(sessionId: string) {
  audioHolds.set(sessionId, (audioHolds.get(sessionId) ?? 0) + 1);
}

export function releaseAudioHold(sessionId: string) {
  const count = audioHolds.get(sessionId);
  if (count === undefined) return;
  if (count <= 1) audioHolds.delete(sessionId);
  else audioHolds.set(sessionId, count - 1);
}

export function isAudioHeld(sessionId: string) {
  return audioHolds.has(sessionId);
}

export function subscribeToSessionAudioRetention(
  listener: (event: SessionAudioRetentionEvent) => void,
) {
  const unlisten = transcriptionEvents.sessionAudioRetentionEvent.listen(
    ({ payload }) => listener(payload),
  );
  return () => {
    void unlisten.then((stop) => stop());
  };
}

export function isSessionAudioIdle(sessionId: string) {
  const state = listenerStore.getState();
  return (
    !isAudioHeld(sessionId) &&
    state.getSessionMode(sessionId) === "inactive" &&
    !(state.live.sessionId === sessionId && state.live.loading)
  );
}

export async function deleteProcessedAudioForRetention(
  policy: AudioRetentionPolicy,
  sessionId: string,
) {
  if (policy !== "none" || !isSessionAudioIdle(sessionId)) {
    return false;
  }

  try {
    const result =
      await transcriptionCommands.deleteProcessedSessionAudio(sessionId);
    if (result.status === "error") {
      throw new Error(result.error);
    }
    return result.data;
  } catch (error) {
    console.error("[audio-retention] failed to delete audio", {
      sessionId,
      error,
    });
    return false;
  }
}
