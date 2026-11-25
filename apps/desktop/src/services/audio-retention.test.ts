import { beforeEach, describe, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  deleteProcessedSessionAudio: vi.fn(),
  getSessionMode: vi.fn(),
  live: { loading: false, sessionId: null as string | null },
}));

vi.mock("@anlg/plugin-transcription", () => ({
  commands: { deleteProcessedSessionAudio: mocks.deleteProcessedSessionAudio },
  events: {},
}));

vi.mock("~/store/zustand/listener/instance", () => ({
  listenerStore: {
    getState: () => ({
      getSessionMode: mocks.getSessionMode,
      live: mocks.live,
    }),
  },
}));

import {
  deleteProcessedAudioForRetention,
  isAudioHeld,
  isSessionAudioIdle,
  normalizeAudioRetention,
  registerAudioHold,
  releaseAudioHold,
} from "./audio-retention";

describe("audio retention", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.deleteProcessedSessionAudio.mockResolvedValue({
      status: "ok",
      data: true,
    });
    mocks.getSessionMode.mockReturnValue("inactive");
    mocks.live.loading = false;
    mocks.live.sessionId = null;
  });

  test("normalizes current and legacy values", () => {
    expect(normalizeAudioRetention("none")).toBe("none");
    expect(normalizeAudioRetention("oneWeek")).toBe("oneWeek");
    expect(normalizeAudioRetention("forever")).toBe("forever");
    expect(normalizeAudioRetention(false)).toBe("none");
    expect(normalizeAudioRetention(true)).toBe("forever");
    expect(normalizeAudioRetention("invalid")).toBe("forever");
    expect(normalizeAudioRetention("invalid", undefined)).toBeUndefined();
  });

  test("does not delete audio while capture startup is loading", async () => {
    mocks.live.loading = true;
    mocks.live.sessionId = "session-1";

    await expect(
      deleteProcessedAudioForRetention("none", "session-1"),
    ).resolves.toBe(false);
    expect(mocks.deleteProcessedSessionAudio).not.toHaveBeenCalled();
  });

  test("does not report a failed native deletion as deleted", async () => {
    mocks.deleteProcessedSessionAudio.mockResolvedValue({
      status: "error",
      error: "disk busy",
    });
    vi.spyOn(console, "error").mockImplementation(() => {});

    await expect(
      deleteProcessedAudioForRetention("none", "session-1"),
    ).resolves.toBe(false);
  });

  test("a held session is not idle and is not deleted until released", async () => {
    registerAudioHold("session-held");
    expect(isAudioHeld("session-held")).toBe(true);
    expect(isSessionAudioIdle("session-held")).toBe(false);
    await expect(
      deleteProcessedAudioForRetention("none", "session-held"),
    ).resolves.toBe(false);
    expect(mocks.deleteProcessedSessionAudio).not.toHaveBeenCalled();

    releaseAudioHold("session-held");
    expect(isSessionAudioIdle("session-held")).toBe(true);
    await expect(
      deleteProcessedAudioForRetention("none", "session-held"),
    ).resolves.toBe(true);
  });

  test("overlapping holds need matching releases", () => {
    registerAudioHold("session-2");
    registerAudioHold("session-2");
    releaseAudioHold("session-2");
    expect(isSessionAudioIdle("session-2")).toBe(false);
    releaseAudioHold("session-2");
    releaseAudioHold("session-2");
    expect(isSessionAudioIdle("session-2")).toBe(true);
  });

  test("days retention is left to the expiry sweep", async () => {
    await expect(
      deleteProcessedAudioForRetention("days:3", "session-1"),
    ).resolves.toBe(false);
    expect(mocks.deleteProcessedSessionAudio).not.toHaveBeenCalled();
  });
});
