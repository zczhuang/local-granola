import { describe, expect, it } from "vitest";

import type { MeetingAccessibilityInspection } from "@anlg/plugin-detect";

import {
  filterNewParticipantNames,
  isConfirmedCallMicApps,
  isGenericSessionTitle,
} from "./meeting-metadata-sync";

function makeInspection(
  overrides: Partial<MeetingAccessibilityInspection> = {},
): MeetingAccessibilityInspection {
  return {
    activeCall: true,
    app: { id: "us.zoom.xos", name: "zoom.us" },
    pid: 123,
    platform: "zoom",
    surface: "native",
    accessibilityTrusted: true,
    windowTitle: "Q4 Planning - Zoom",
    meetingTitle: "Q4 Planning",
    participants: ["Ada Lovelace", "Grace Hopper"],
    activeSpeaker: null,
    remoteScreenShare: false,
    warnings: [],
    ...overrides,
  };
}

describe("meeting-metadata-sync", () => {
  it("recognizes generic placeholder titles vs meaningful meeting topics", () => {
    expect(isGenericSessionTitle("")).toBe(true);
    expect(isGenericSessionTitle("Zoom Meeting")).toBe(true);
    expect(isGenericSessionTitle("Google Meet")).toBe(true);
    expect(isGenericSessionTitle("Q4 Roadmap Review")).toBe(false);
  });

  it("filters out self names, duplicates, and already-attached participants", () => {
    const fresh = filterNewParticipantNames({
      detectedNames: [
        "Stuart Chuang",
        "Ada Lovelace",
        "ada lovelace",
        "Grace Hopper",
        "You",
        "Linus Torvalds",
      ],
      selfNames: ["Stuart Chuang"],
      existingParticipantNames: ["Grace Hopper"],
      attemptedNames: new Set(["linus torvalds"]),
    });

    expect(fresh).toEqual(["Ada Lovelace"]);
  });

  it("confirms dedicated meeting apps and verified browser calls while ignoring random browser mic use", () => {
    expect(
      isConfirmedCallMicApps(
        [{ id: "us.zoom.xos", name: "zoom.us" }],
        [],
        false,
      ),
    ).toBe(true);

    expect(
      isConfirmedCallMicApps(
        [{ id: "com.google.Chrome", name: "Google Chrome" }],
        [],
        false,
      ),
    ).toBe(false);

    expect(
      isConfirmedCallMicApps(
        [{ id: "com.google.Chrome", name: "Google Chrome" }],
        [
          makeInspection({
            app: { id: "com.google.Chrome", name: "Google Chrome" },
            platform: "googleMeet",
            surface: "web",
            windowTitle: "Meet – Weekly Sync",
          }),
        ],
        false,
      ),
    ).toBe(true);

    expect(
      isConfirmedCallMicApps(
        [{ id: "com.google.Chrome", name: "Google Chrome" }],
        [],
        true,
      ),
    ).toBe(true);
  });
});
