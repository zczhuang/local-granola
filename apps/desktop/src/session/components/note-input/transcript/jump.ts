// LOCAL-GRANOLA: click-to-jump from an AI line of the enhanced note to the
// transcript segments it was derived from.
//
// AI lines carry `src` = [{ t: transcriptId, s: startWordId, e: endWordId,
// a: startMs, b: endMs }] (see crates/enhanced-notes render.rs). Word ids are
// stable across speaker re-labeling, so segments are matched by the word ids
// they contain, not by rendered segment ids.

import { useSyncExternalStore } from "react";

import { type Tab, useTabs } from "~/store/zustand/tabs";

export type AiLineSource = {
  t: string;
  s: string;
  e: string;
  a?: number;
  b?: number;
};

type SegmentLike = { words: ReadonlyArray<{ id?: string | null }> };

/** Parse the `data-src` attribute / `src` node attr of an AI line. */
export function parseAiLineSources(raw: unknown): AiLineSource[] {
  let value = raw;
  if (typeof raw === "string") {
    try {
      value = JSON.parse(raw);
    } catch {
      return [];
    }
  }
  if (!Array.isArray(value)) return [];
  return value.filter(
    (item): item is AiLineSource =>
      !!item &&
      typeof item === "object" &&
      typeof (item as AiLineSource).s === "string" &&
      typeof (item as AiLineSource).e === "string",
  );
}

function indexOfWord(segments: readonly SegmentLike[], wordId: string) {
  if (!wordId) return -1;
  return segments.findIndex((segment) =>
    segment.words.some((word) => word.id === wordId),
  );
}

/**
 * Index of the first segment containing the source's start word (falling back
 * to its end word), or null when the source is not in these segments.
 */
export function resolveSourceToSegmentIndex(
  src: Pick<AiLineSource, "s" | "e">,
  segments: readonly SegmentLike[],
): number | null {
  const start = indexOfWord(segments, src.s);
  if (start >= 0) return start;
  const end = indexOfWord(segments, src.e);
  return end >= 0 ? end : null;
}

/** Every segment from the source's start word through its end word. */
export function resolveSourceToSegmentIndexes(
  src: Pick<AiLineSource, "s" | "e">,
  segments: readonly SegmentLike[],
): number[] {
  const first = resolveSourceToSegmentIndex(src, segments);
  if (first === null) return [];
  const end = indexOfWord(segments, src.e);
  const last = end >= first ? end : first;
  return Array.from({ length: last - first + 1 }, (_, i) => first + i);
}

/** Union of indexes (sorted, unique) for several sources of one transcript. */
export function resolveSourcesToSegmentIndexes(
  sources: readonly AiLineSource[],
  segments: readonly SegmentLike[],
  transcriptId?: string,
): number[] {
  const indexes = new Set<number>();
  for (const src of sources) {
    if (transcriptId && src.t && src.t !== transcriptId) continue;
    for (const index of resolveSourceToSegmentIndexes(src, segments)) {
      indexes.add(index);
    }
  }
  return Array.from(indexes).sort((a, b) => a - b);
}

export type TranscriptJumpRequest = {
  sessionId: string;
  sources: AiLineSource[];
  nonce: number;
};

let current: TranscriptJumpRequest | null = null;
let nonce = 0;
const listeners = new Set<() => void>();

export function requestTranscriptJump(
  sessionId: string,
  sources: AiLineSource[],
): TranscriptJumpRequest | null {
  if (sources.length === 0) return null;
  current = { sessionId, sources, nonce: ++nonce };
  listeners.forEach((listener) => listener());
  return current;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useTranscriptJumpRequest(
  sessionId: string,
): TranscriptJumpRequest | null {
  const request = useSyncExternalStore(
    subscribe,
    () => current,
    () => null,
  );
  return request && request.sessionId === sessionId ? request : null;
}

/** Switch the session's tab to its transcript and request the jump. */
export function jumpToTranscriptSources(
  sessionId: string,
  sources: AiLineSource[],
) {
  if (sources.length === 0) return;
  const tabs = useTabs.getState();
  const sessionTab = tabs.tabs.find(
    (tab): tab is Extract<Tab, { type: "sessions" }> =>
      tab.type === "sessions" && tab.id === sessionId,
  );
  if (sessionTab && sessionTab.state.view?.type !== "transcript") {
    tabs.updateSessionTabState(sessionTab, {
      ...sessionTab.state,
      view: { type: "transcript" },
    });
  }
  requestTranscriptJump(sessionId, sources);
}
