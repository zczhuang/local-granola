import { beforeEach, expect, it, vi } from "vitest";

import type { TaskArgsMapTransformed } from ".";
import { executeLocalEnhance } from "./enhance-local";
import { enhanceWorkflow } from "./enhance-workflow";
import { resolveEnhanceRoute } from "./local-route";

const mocks = vi.hoisted(() => ({
  enhanceLocal: vi.fn(),
  render: vi.fn(),
  streamText: vi.fn(),
}));
vi.mock("@anlg/plugin-template", () => ({
  commands: { enhanceLocal: mocks.enhanceLocal, render: mocks.render },
}));
vi.mock("~/shared/local-only", async (importOriginal) => ({
  ...(await importOriginal<typeof import("~/shared/local-only")>()),
  // The test env runs hosted (VITE_LOCAL_ONLY=false); these tests cover local.
  LOCAL_ONLY: true,
}));
vi.mock("ai", async (importOriginal) => ({
  ...(await importOriginal<typeof import("ai")>()),
  streamText: mocks.streamText,
}));

beforeEach(() => {
  vi.clearAllMocks();
});

it("routes local-only builds to enhance_local and hosted builds to streaming", () => {
  expect(resolveEnhanceRoute(true)).toBe("local");
  expect(resolveEnhanceRoute(false)).toBe("stream");
});

const localEnhance = {
  sessionId: "session-1",
  enhancedNoteId: "note-1",
  regenerate: false,
};

async function collect(stream: AsyncIterable<any>) {
  const chunks: any[] = [];
  for await (const chunk of stream) chunks.push(chunk);
  return chunks;
}

it("calls enhance_local (and never the LLM stream) in LOCAL_ONLY builds", async () => {
  mocks.enhanceLocal.mockResolvedValue({
    status: "ok",
    data: { text: "- Starter plan is $10", note_id: "note-1" },
  });

  const chunks = await collect(
    enhanceWorkflow.executeWorkflow({
      model: {} as never,
      args: { localEnhance } as TaskArgsMapTransformed["enhance"],
      onProgress: vi.fn(),
      signal: new AbortController().signal,
    }),
  );

  expect(mocks.enhanceLocal).toHaveBeenCalledWith(
    expect.objectContaining({
      session_id: "session-1",
      note_id: "note-1",
      regenerate: false,
      pending_auto_enhance: null,
    }),
  );
  expect(mocks.streamText).not.toHaveBeenCalled();
  expect(mocks.render).not.toHaveBeenCalled();
  expect(chunks).toEqual([
    { type: "text-delta", id: "enhance-local", text: "- Starter plan is $10" },
  ]);
});

it("passes regenerate and the pending auto-enhance guard through", async () => {
  mocks.enhanceLocal.mockResolvedValue({
    status: "ok",
    data: { text: "x", note_id: "note-1" },
  });

  await collect(
    executeLocalEnhance({
      args: {
        ...localEnhance,
        regenerate: true,
        pendingAutoEnhance: {
          generation: "g1",
          expectedBody: "body",
          expectedContentFormat: "markdown",
        },
      },
      onProgress: vi.fn(),
      signal: new AbortController().signal,
    }),
  );

  expect(mocks.enhanceLocal).toHaveBeenCalledWith(
    expect.objectContaining({
      regenerate: true,
      pending_auto_enhance: {
        generation: "g1",
        expected_body: "body",
        expected_body_format: "markdown",
      },
    }),
  );
});

it("surfaces command errors", async () => {
  mocks.enhanceLocal.mockResolvedValue({
    status: "error",
    error: "model request failed",
  });
  await expect(
    collect(
      executeLocalEnhance({
        args: localEnhance,
        onProgress: vi.fn(),
        signal: new AbortController().signal,
      }),
    ),
  ).rejects.toThrow("model request failed");
});

it("emits heartbeats while a long enhancement runs", async () => {
  vi.useFakeTimers();
  try {
    let resolve!: (value: unknown) => void;
    mocks.enhanceLocal.mockReturnValue(
      new Promise((r) => {
        resolve = r;
      }),
    );
    const iterator = executeLocalEnhance({
      args: localEnhance,
      onProgress: vi.fn(),
      signal: new AbortController().signal,
    })[Symbol.asyncIterator]();

    const first = iterator.next();
    await vi.advanceTimersByTimeAsync(10_000);
    expect((await first).value).toEqual({ type: "raw", rawValue: "heartbeat" });

    const second = iterator.next();
    resolve({ status: "ok", data: { text: "done", note_id: "note-1" } });
    expect((await second).value).toMatchObject({ text: "done" });
  } finally {
    vi.useRealTimers();
  }
});
