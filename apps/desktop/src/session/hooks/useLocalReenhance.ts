// LOCAL-GRANOLA: when the user edits their typed notes after a summary was
// generated, re-run the local enhancer once they pause. AI lines they edited
// or deleted are kept (reconcile in crates/enhanced-notes); only the
// "Regenerate" action discards those edits.

import { useEffect, useRef } from "react";

import { commands as templateCommands } from "@anlg/plugin-template";

import { getEnhancerService } from "~/services/enhancer";
import { shouldEnhanceLocally } from "~/store/zustand/ai-task/task-configs/local-route";

export const LOCAL_REENHANCE_DEBOUNCE_MS = 20_000;

export async function reenhanceIfStale(
  sessionId: string,
  noteId: string,
): Promise<boolean> {
  const result = await templateCommands.enhanceLocalIsStale({
    session_id: sessionId,
    note_id: noteId,
  });
  if (result.status === "error" || !result.data) {
    return false;
  }
  const outcome = await getEnhancerService()?.enhance(sessionId, {
    targetNoteId: noteId,
    localRefresh: true,
  });
  return outcome?.type === "started";
}

export function useLocalReenhance({
  sessionId,
  rawContent,
  noteId,
  enabled,
  debounceMs = LOCAL_REENHANCE_DEBOUNCE_MS,
}: {
  sessionId: string;
  /** The raw (typed) note body; any change restarts the debounce. */
  rawContent: string | null | undefined;
  /** The enhanced note to refresh; none means nothing to re-enhance. */
  noteId: string | null | undefined;
  enabled: boolean;
  debounceMs?: number;
}) {
  const lastRawRef = useRef<string | null | undefined>(undefined);
  const noteIdRef = useRef(noteId);
  noteIdRef.current = noteId;
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;

  useEffect(() => {
    // The first value is the baseline, not an edit.
    if (lastRawRef.current === undefined) {
      lastRawRef.current = rawContent ?? "";
      return;
    }
    if (rawContent == null || rawContent === lastRawRef.current) {
      return;
    }
    lastRawRef.current = rawContent;
    if (!shouldEnhanceLocally()) {
      return;
    }

    const timer = setTimeout(() => {
      const target = noteIdRef.current;
      if (!target || !enabledRef.current) return;
      void reenhanceIfStale(sessionId, target).catch((error) => {
        console.error("[enhancer] local re-enhance failed", error);
      });
    }, debounceMs);
    return () => clearTimeout(timer);
  }, [rawContent, sessionId, debounceMs]);
}
