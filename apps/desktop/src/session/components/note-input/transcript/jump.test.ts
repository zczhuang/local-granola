import { describe, expect, it } from "vitest";

import {
  parseAiLineSources,
  resolveSourcesToSegmentIndexes,
  resolveSourceToSegmentIndex,
  resolveSourceToSegmentIndexes,
} from "./jump";

const segments = [
  { words: [{ id: "w1" }, { id: "w2" }] },
  { words: [{ id: "w3" }, { id: null }, { id: "w4" }] },
  { words: [{ id: "w5" }] },
];

describe("resolveSourceToSegmentIndex", () => {
  it("finds the segment containing the start word", () => {
    expect(resolveSourceToSegmentIndex({ s: "w3", e: "w4" }, segments)).toBe(1);
    expect(resolveSourceToSegmentIndex({ s: "w1", e: "w5" }, segments)).toBe(0);
  });

  it("falls back to the end word when the start word is gone", () => {
    expect(resolveSourceToSegmentIndex({ s: "gone", e: "w5" }, segments)).toBe(
      2,
    );
  });

  it("returns null when neither word exists", () => {
    expect(
      resolveSourceToSegmentIndex({ s: "x", e: "y" }, segments),
    ).toBeNull();
    expect(resolveSourceToSegmentIndex({ s: "", e: "" }, segments)).toBeNull();
    expect(resolveSourceToSegmentIndex({ s: "w1", e: "w1" }, [])).toBeNull();
  });
});

describe("resolveSourceToSegmentIndexes", () => {
  it("covers every segment from start to end word", () => {
    expect(
      resolveSourceToSegmentIndexes({ s: "w2", e: "w5" }, segments),
    ).toEqual([0, 1, 2]);
  });

  it("is a single segment when start and end share one", () => {
    expect(
      resolveSourceToSegmentIndexes({ s: "w3", e: "w4" }, segments),
    ).toEqual([1]);
  });

  it("does not run backwards when the end word precedes the start", () => {
    expect(
      resolveSourceToSegmentIndexes({ s: "w5", e: "w1" }, segments),
    ).toEqual([2]);
  });
});

describe("resolveSourcesToSegmentIndexes", () => {
  it("unions sources and ignores other transcripts", () => {
    const sources = [
      { t: "t1", s: "w1", e: "w1" },
      { t: "t2", s: "w5", e: "w5" },
      { t: "t1", s: "w4", e: "w4" },
    ];
    expect(resolveSourcesToSegmentIndexes(sources, segments, "t1")).toEqual([
      0, 1,
    ]);
    expect(resolveSourcesToSegmentIndexes(sources, segments)).toEqual([
      0, 1, 2,
    ]);
  });
});

describe("parseAiLineSources", () => {
  it("parses JSON strings and arrays, dropping malformed entries", () => {
    const src = [{ t: "t1", s: "w1", e: "w2", a: 0, b: 10 }];
    expect(parseAiLineSources(JSON.stringify(src))).toEqual(src);
    expect(parseAiLineSources(src)).toEqual(src);
    expect(parseAiLineSources([{ t: "t1" }, null, 3])).toEqual([]);
    expect(parseAiLineSources("not json")).toEqual([]);
    expect(parseAiLineSources(null)).toEqual([]);
  });
});
