import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import {
  LOCAL_REENHANCE_DEBOUNCE_MS,
  useLocalReenhance,
} from "./useLocalReenhance";

const mocks = vi.hoisted(() => ({
  isStale: vi.fn(),
  enhance: vi.fn(),
}));
vi.mock("@anlg/plugin-template", () => ({
  commands: { enhanceLocalIsStale: mocks.isStale },
}));
vi.mock("~/shared/local-only", async (importOriginal) => ({
  ...(await importOriginal<typeof import("~/shared/local-only")>()),
  // The test env runs hosted (VITE_LOCAL_ONLY=false); these tests cover local.
  LOCAL_ONLY: true,
}));
vi.mock("~/services/enhancer", () => ({
  getEnhancerService: () => ({ enhance: mocks.enhance }),
}));

beforeEach(() => {
  vi.useFakeTimers();
  mocks.isStale.mockResolvedValue({ status: "ok", data: true });
  mocks.enhance.mockResolvedValue({ type: "started", noteId: "note-1" });
});
afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

function setup(initial: string) {
  return renderHook(
    (props: { raw: string }) =>
      useLocalReenhance({
        sessionId: "s1",
        rawContent: props.raw,
        noteId: "note-1",
        enabled: true,
      }),
    { initialProps: { raw: initial } },
  );
}

it("does nothing for the initial value", async () => {
  setup("a");
  await vi.advanceTimersByTimeAsync(LOCAL_REENHANCE_DEBOUNCE_MS * 2);
  expect(mocks.isStale).not.toHaveBeenCalled();
});

it("re-enhances once after the user stops editing, keeping AI edits", async () => {
  const view = setup("a");
  view.rerender({ raw: "ab" });
  await vi.advanceTimersByTimeAsync(LOCAL_REENHANCE_DEBOUNCE_MS - 1_000);
  view.rerender({ raw: "abc" });
  await vi.advanceTimersByTimeAsync(LOCAL_REENHANCE_DEBOUNCE_MS - 1_000);
  expect(mocks.isStale).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1_000);

  expect(mocks.isStale).toHaveBeenCalledTimes(1);
  expect(mocks.enhance).toHaveBeenCalledWith("s1", {
    targetNoteId: "note-1",
    localRefresh: true,
  });
  expect(mocks.enhance.mock.calls[0]![1]).not.toHaveProperty("regenerate");
});

it("skips the enhance when the summary is already current", async () => {
  mocks.isStale.mockResolvedValue({ status: "ok", data: false });
  const view = setup("a");
  view.rerender({ raw: "a " });
  await vi.advanceTimersByTimeAsync(LOCAL_REENHANCE_DEBOUNCE_MS);
  expect(mocks.isStale).toHaveBeenCalledTimes(1);
  expect(mocks.enhance).not.toHaveBeenCalled();
});
