import { beforeEach, describe, expect, test, vi } from "vitest";

const { decideSttRoute } = vi.hoisted(() => ({ decideSttRoute: vi.fn() }));
vi.mock("@anlg/plugin-transcription", () => ({
  commands: { decideSttRoute },
}));

import {
  clearForcedRepass,
  createEngineRoutingProbe,
  forcedRepassFor,
  getForcedRepass,
  normalizeSttEngineOverride,
} from "./engine-routing";

const LIVE = "soniqo-parakeet-v3-live";

beforeEach(() => {
  vi.clearAllMocks();
  clearForcedRepass("s1");
});

describe("forcedRepassFor", () => {
  test("Whisper verdict on auto + fast local engine forces the re-pass", () => {
    expect(
      forcedRepassFor({ engine: "whisper", lang: "zh" }, "auto", LIVE),
    ).toEqual({ languages: ["zh"] });
    expect(
      forcedRepassFor({ engine: "whisper", lang: null }, undefined, LIVE),
    ).toEqual({ languages: [] });
  });

  test.each([
    [{ engine: "parakeet" }, "auto", LIVE],
    [{ engine: "pending" }, "auto", LIVE],
    [{ engine: "whisper", lang: "zh" }, "parakeet", LIVE],
    [{ engine: "whisper", lang: "zh" }, "whisper", LIVE],
    [{ engine: "whisper", lang: "zh" }, "auto", "QuantizedLargeTurbo"],
    [{ engine: "whisper", lang: "zh" }, "auto", "cloud"],
  ] as const)(
    "%j override %s engine %s does nothing",
    (decision, o, engine) => {
      expect(forcedRepassFor(decision as never, o, engine)).toBeNull();
    },
  );

  test("normalizes the override", () => {
    expect(normalizeSttEngineOverride("whisper")).toBe("whisper");
    expect(normalizeSttEngineOverride("x")).toBe("auto");
  });
});

describe("createEngineRoutingProbe", () => {
  const words = (ms: number) => ({
    new_words: [{ text: " 你好", start_ms: 0, end_ms: ms }],
  });
  const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

  test("asks the router once after 30 s of speech and marks the session", async () => {
    decideSttRoute.mockResolvedValue({
      status: "ok",
      data: { engine: "whisper", lang: "zh" },
    });
    const probe = createEngineRoutingProbe({
      sessionId: "s1",
      override: "auto",
      liveEngine: LIVE,
      declaredLanguages: ["zh"],
    });
    probe.observe(words(20_000));
    expect(decideSttRoute).not.toHaveBeenCalled();
    probe.observe(words(10_000));
    probe.observe(words(10_000));
    await flush();
    expect(decideSttRoute).toHaveBeenCalledTimes(1);
    expect(decideSttRoute).toHaveBeenCalledWith({
      preference: "auto",
      declared_languages: ["zh"],
      sample_text: " 你好 你好",
      speech_ms: 30_000,
    });
    expect(getForcedRepass("s1")).toEqual({ languages: ["zh"] });
  });

  test("a Parakeet verdict leaves the session alone", async () => {
    decideSttRoute.mockResolvedValue({
      status: "ok",
      data: { engine: "parakeet" },
    });
    const probe = createEngineRoutingProbe({
      sessionId: "s1",
      override: "auto",
      liveEngine: LIVE,
      declaredLanguages: ["en"],
    });
    probe.observe(words(31_000));
    await flush();
    expect(getForcedRepass("s1")).toBeNull();
    probe.observe(words(31_000));
    expect(decideSttRoute).toHaveBeenCalledTimes(1);
  });

  test("an error result marks nothing", async () => {
    decideSttRoute.mockResolvedValue({ status: "error", error: "boom" });
    const probe = createEngineRoutingProbe({
      sessionId: "s1",
      override: "auto",
      liveEngine: LIVE,
      declaredLanguages: ["zh"],
    });
    probe.observe(words(31_000));
    await flush();
    expect(getForcedRepass("s1")).toBeNull();
  });

  test("pending verdict asks again after another 30 s", async () => {
    decideSttRoute.mockResolvedValue({
      status: "ok",
      data: { engine: "pending" },
    });
    const probe = createEngineRoutingProbe({
      sessionId: "s1",
      override: "auto",
      liveEngine: LIVE,
      declaredLanguages: [],
    });
    probe.observe(words(31_000));
    await flush();
    probe.observe(words(10_000));
    expect(decideSttRoute).toHaveBeenCalledTimes(1);
    probe.observe(words(25_000));
    await flush();
    expect(decideSttRoute).toHaveBeenCalledTimes(2);
  });

  test("explicit override or non-local live engine never asks", () => {
    for (const [override, engine] of [
      ["whisper", LIVE],
      ["parakeet", LIVE],
      ["auto", "cloud"],
    ] as const) {
      createEngineRoutingProbe({
        sessionId: "s1",
        override,
        liveEngine: engine,
        declaredLanguages: ["zh"],
      }).observe(words(60_000));
    }
    expect(decideSttRoute).not.toHaveBeenCalled();
  });

  test("a new probe clears a stale mark from an earlier recording", () => {
    decideSttRoute.mockResolvedValue({
      status: "ok",
      data: { engine: "whisper", lang: null },
    });
    createEngineRoutingProbe({
      sessionId: "s1",
      override: "auto",
      liveEngine: LIVE,
      declaredLanguages: [],
    });
    expect(getForcedRepass("s1")).toBeNull();
  });
});
