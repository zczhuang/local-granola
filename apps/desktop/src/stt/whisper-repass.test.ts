import { beforeEach, describe, expect, test, vi } from "vitest";

const { isModelDownloaded, startServer } = vi.hoisted(() => ({
  isModelDownloaded: vi.fn(),
  startServer: vi.fn(),
}));

vi.mock("@anlg/plugin-local-stt", () => ({
  commands: { isModelDownloaded, startServer },
}));
const {
  deleteProcessedSessionAudio,
  acquireSessionAudioHold,
  dropSessionAudioHold,
} = vi.hoisted(() => ({
  deleteProcessedSessionAudio: vi.fn(),
  acquireSessionAudioHold: vi.fn(),
  dropSessionAudioHold: vi.fn(),
}));
vi.mock("@anlg/plugin-transcription", () => ({
  commands: {
    deleteProcessedSessionAudio,
    acquireSessionAudioHold,
    dropSessionAudioHold,
  },
  events: {},
}));
vi.mock("~/store/zustand/listener/instance", () => ({
  listenerStore: {
    getState: () => ({
      getSessionMode: () => "inactive",
      live: { sessionId: null, loading: false },
    }),
  },
}));

import {
  normalizeWhisperRepass,
  resolveWhisperRepassTarget,
  shouldRepass,
  withWhisperRepassHold,
} from "./whisper-repass";

import { isAudioHeld } from "~/services/audio-retention";

beforeEach(() => {
  vi.clearAllMocks();
  acquireSessionAudioHold.mockResolvedValue({ status: "ok", data: null });
  dropSessionAudioHold.mockResolvedValue({ status: "ok", data: null });
});

describe("shouldRepass", () => {
  test.each([
    ["off", "soniqo-parakeet-v3-live", false],
    ["auto", "soniqo-parakeet-v3-live", true],
    ["auto", "soniqo-parakeet-streaming", true],
    ["auto", "apple-speech", true],
    ["auto", "QuantizedLargeTurbo", false],
    ["auto", "cloud", false],
    ["auto", undefined, false],
    ["always", "cloud", true],
    ["always", undefined, true],
    ["bogus", "soniqo-parakeet-v3-live", true],
    [undefined, "cloud", false],
  ])("setting %s engine %s -> %s", (setting, engine, expected) => {
    expect(shouldRepass(setting, engine)).toBe(expected);
  });

  test.each([
    ["off", "soniqo-parakeet-v3-live", false],
    ["auto", "cloud", true],
    ["auto", "QuantizedLargeTurbo", true],
    ["always", "cloud", true],
  ])("forced: setting %s engine %s -> %s", (setting, engine, expected) => {
    expect(shouldRepass(setting, engine, true)).toBe(expected);
  });

  test("normalize falls back to auto", () => {
    expect(normalizeWhisperRepass("nope")).toBe("auto");
    expect(normalizeWhisperRepass("off")).toBe("off");
  });
});

describe("withWhisperRepassHold", () => {
  test("holds audio during the run and releases after success or failure", async () => {
    deleteProcessedSessionAudio.mockResolvedValue({ status: "ok", data: true });
    await withWhisperRepassHold("s1", "none", async () => {
      expect(isAudioHeld("s1")).toBe(true);
      expect(deleteProcessedSessionAudio).not.toHaveBeenCalled();
    });
    expect(isAudioHeld("s1")).toBe(false);
    // The retention policy is applied again once the hold is gone.
    expect(deleteProcessedSessionAudio).toHaveBeenCalledWith("s1");
    deleteProcessedSessionAudio.mockClear();
    await expect(
      withWhisperRepassHold("s1", "forever" as never, async () => {
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");
    expect(isAudioHeld("s1")).toBe(false);
    expect(deleteProcessedSessionAudio).not.toHaveBeenCalled();
  });
});

describe("withWhisperRepassHold plugin hold", () => {
  test("holds in Rust before the run and releases it before re-applying retention", async () => {
    const order: string[] = [];
    acquireSessionAudioHold.mockImplementation(async () => {
      order.push("acquire");
      return { status: "ok", data: null };
    });
    dropSessionAudioHold.mockImplementation(async () => {
      order.push("drop");
      return { status: "ok", data: null };
    });
    deleteProcessedSessionAudio.mockImplementation(async () => {
      order.push("delete");
      return { status: "ok", data: true };
    });
    await withWhisperRepassHold("s2", "none", async () => {
      order.push("run");
    });
    expect(order).toEqual(["acquire", "run", "drop", "delete"]);
    expect(acquireSessionAudioHold).toHaveBeenCalledWith("s2");
    expect(dropSessionAudioHold).toHaveBeenCalledWith("s2");
  });

  test("releases the Rust hold when the run throws", async () => {
    await expect(
      withWhisperRepassHold("s3", "forever" as never, async () => {
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");
    expect(dropSessionAudioHold).toHaveBeenCalledWith("s3");
  });

  test.each([
    [
      "rejects",
      () => acquireSessionAudioHold.mockRejectedValue(new Error("x")),
    ],
    [
      "returns an error",
      () =>
        acquireSessionAudioHold.mockResolvedValue({
          status: "error",
          error: "x",
        }),
    ],
  ])(
    "still runs (and does not release a hold it never took) when acquiring %s",
    async (_name, arrange) => {
      arrange();
      const run = vi.fn().mockResolvedValue("done");
      await expect(
        withWhisperRepassHold("s4", "forever" as never, run),
      ).resolves.toBe("done");
      expect(run).toHaveBeenCalled();
      expect(dropSessionAudioHold).not.toHaveBeenCalled();
      expect(isAudioHeld("s4")).toBe(false);
    },
  );
});

describe("resolveWhisperRepassTarget", () => {
  test("returns the local server for the turbo model", async () => {
    isModelDownloaded.mockResolvedValue({ status: "ok", data: true });
    startServer.mockResolvedValue({ status: "ok", data: "http://127.0.0.1:1" });
    await expect(resolveWhisperRepassTarget()).resolves.toEqual({
      provider: "anarlog",
      model: "QuantizedLargeTurbo",
      baseUrl: "http://127.0.0.1:1",
      apiKey: "",
    });
  });

  test("returns null when the model is not downloaded", async () => {
    isModelDownloaded.mockResolvedValue({ status: "ok", data: false });
    await expect(resolveWhisperRepassTarget()).resolves.toBeNull();
    expect(startServer).not.toHaveBeenCalled();
  });

  test("returns null when the server fails to start", async () => {
    isModelDownloaded.mockResolvedValue({ status: "ok", data: true });
    startServer.mockResolvedValue({ status: "error", error: "x" });
    await expect(resolveWhisperRepassTarget()).resolves.toBeNull();
  });
});
