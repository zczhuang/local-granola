import { describe, expect, test } from "vitest";

import {
  audioRetentionDays,
  controlsToRetention,
  normalizeAudioRetention,
  parseAudioRetentionDays,
  retentionToControls,
  shouldRetainAudio,
} from "./audio-retention-policy";

describe("days retention", () => {
  test("parses days:N within 1..3650 only", () => {
    expect(parseAudioRetentionDays("days:14")).toBe(14);
    expect(parseAudioRetentionDays("days:3650")).toBe(3650);
    for (const bad of [
      "days:0",
      "days:3651",
      "days:",
      "days:-1",
      "days:1.5",
      "days: 2",
      "Days:2",
      "days:99999999999999999999",
      14,
      null,
    ]) {
      expect(parseAudioRetentionDays(bad)).toBeNull();
    }
  });

  test("normalizes days values and rejects malformed ones", () => {
    expect(normalizeAudioRetention("days:7")).toBe("days:7");
    expect(normalizeAudioRetention("days:0", undefined)).toBeUndefined();
    expect(normalizeAudioRetention("days:x")).toBe("forever");
  });

  test("audioRetentionDays truncates and bounds", () => {
    expect(audioRetentionDays(3.9)).toBe("days:3");
    expect(audioRetentionDays(0)).toBeNull();
    expect(audioRetentionDays(4000)).toBeNull();
  });
});

describe("toggle and number-field mapping", () => {
  test("maps every policy to controls", () => {
    expect(retentionToControls("forever")).toEqual({
      deleteAfter: false,
      days: null,
    });
    expect(retentionToControls("none")).toEqual({
      deleteAfter: true,
      days: null,
    });
    expect(retentionToControls("oneWeek")).toEqual({
      deleteAfter: true,
      days: 7,
    });
    expect(retentionToControls("days:12")).toEqual({
      deleteAfter: true,
      days: 12,
    });
  });

  test("maps controls back to a policy", () => {
    expect(controlsToRetention({ deleteAfter: false, days: 5 })).toBe(
      "forever",
    );
    expect(controlsToRetention({ deleteAfter: true, days: null })).toBe("none");
    expect(controlsToRetention({ deleteAfter: true, days: 5 })).toBe("days:5");
    expect(controlsToRetention({ deleteAfter: true, days: 0 })).toBe("none");
    expect(controlsToRetention({ deleteAfter: true, days: NaN })).toBe("none");
  });
});

describe("shouldRetainAudio", () => {
  test("local-only always records, even with the default none policy", () => {
    expect(
      shouldRetainAudio({
        localOnly: true,
        audioRetention: "none",
        requiresBatchAudio: false,
      }),
    ).toBe(true);
  });

  test("hosted builds record only when retained or needed for batch", () => {
    const base = { localOnly: false, requiresBatchAudio: false };
    expect(shouldRetainAudio({ ...base, audioRetention: "none" })).toBe(false);
    expect(shouldRetainAudio({ ...base, audioRetention: "oneDay" })).toBe(true);
    expect(
      shouldRetainAudio({
        ...base,
        audioRetention: "none",
        requiresBatchAudio: true,
      }),
    ).toBe(true);
  });
});
