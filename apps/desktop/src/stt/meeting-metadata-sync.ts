import type { MeetingAccessibilityInspection } from "@anlg/plugin-detect";

import { createHuman } from "~/contacts/queries";
import { liveQueryClient } from "~/db";
import { addSessionParticipant, updateSession } from "~/session/queries";
import { hasLiveSessionTitleDraft } from "~/store/zustand/live-title";
import { listenerStore } from "~/store/zustand/listener/instance";
import { inspectionShowsActiveMeeting } from "~/stt/meeting-accessibility";
import {
  BROWSER_AUTO_STOP_APP_IDS,
  isMeetingCapableMicApp,
  type MicApp,
} from "~/stt/meeting-apps";

const GENERIC_SESSION_TITLES = new Set([
  "",
  "untitled",
  "new note",
  "meeting",
  "call",
  "zoom",
  "zoom meeting",
  "zoom workplace",
  "google meet",
  "meet",
  "microsoft teams",
  "teams",
  "webex",
  "slack",
  "slack huddle",
  "huddle",
  "discord",
]);

type SessionSyncState = {
  attemptedParticipants: Set<string>;
  appliedTitle: boolean;
};

const sessionSyncStates = new Map<string, SessionSyncState>();

function getSessionSyncState(sessionId: string): SessionSyncState {
  let state = sessionSyncStates.get(sessionId);
  if (!state) {
    state = { attemptedParticipants: new Set(), appliedTitle: false };
    sessionSyncStates.set(sessionId, state);
  }
  return state;
}

export function clearSessionMetadataSyncState(sessionId: string) {
  sessionSyncStates.delete(sessionId);
}

export function isGenericSessionTitle(title: string | null | undefined): boolean {
  return GENERIC_SESSION_TITLES.has((title ?? "").trim().toLowerCase());
}

export function filterNewParticipantNames({
  detectedNames,
  selfNames,
  existingParticipantNames,
  attemptedNames,
}: {
  detectedNames: readonly string[];
  selfNames: readonly string[];
  existingParticipantNames: readonly string[];
  attemptedNames?: ReadonlySet<string>;
}): string[] {
  const excluded = new Set<string>(["you", "me", "yourself"]);
  for (const name of selfNames) {
    const normalized = name.trim().toLowerCase();
    if (normalized) excluded.add(normalized);
  }
  for (const name of existingParticipantNames) {
    const normalized = name.trim().toLowerCase();
    if (normalized) excluded.add(normalized);
  }

  const seen = new Set<string>();
  const fresh: string[] = [];
  for (const raw of detectedNames) {
    const cleaned = raw.trim().replace(/\s+/g, " ");
    if (cleaned.length < 2) continue;
    const key = cleaned.toLowerCase();
    if (excluded.has(key) || attemptedNames?.has(key) || seen.has(key)) {
      continue;
    }
    seen.add(key);
    fresh.push(cleaned);
  }
  return fresh;
}

export function isConfirmedCallMicApps(
  apps: readonly MicApp[],
  inspections: readonly MeetingAccessibilityInspection[],
  hasNearbyCalendarEvent: boolean,
): boolean {
  const activeInspectionAppIds = new Set(
    inspections
      .filter((inspection) => inspectionShowsActiveMeeting(inspection))
      .map((inspection) => inspection.app.id),
  );

  return apps.some((app) => {
    if (!isMeetingCapableMicApp(app)) {
      return false;
    }
    if (!BROWSER_AUTO_STOP_APP_IDS.has(app.id)) {
      // Dedicated calling / meeting apps (Zoom, Teams, Webex, FaceTime, Slack, etc.)
      return true;
    }
    // Browser apps require positive active-call AX evidence or a nearby calendar meeting
    // so generic browser mic use (e.g. dictation or voice chat) never false-triggers.
    return activeInspectionAppIds.has(app.id) || hasNearbyCalendarEvent;
  });
}

export function reconcileRecordingTriggerApps(
  sessionId: string,
  micApps: readonly MicApp[],
  inspections: readonly MeetingAccessibilityInspection[],
): void {
  const live = listenerStore.getState().live;
  if (live.status !== "active" || live.sessionId !== sessionId) {
    return;
  }

  const activeInspectionAppIds = new Set(
    inspections
      .filter((inspection) => inspectionShowsActiveMeeting(inspection))
      .map((inspection) => inspection.app.id),
  );
  const callAppIds = micApps
    .filter((app) => {
      if (!app.id || app.id.startsWith("pid:")) return false;
      if (!isMeetingCapableMicApp(app)) return false;
      if (!BROWSER_AUTO_STOP_APP_IDS.has(app.id)) return true;
      return activeInspectionAppIds.has(app.id);
    })
    .map((app) => app.id);

  if (callAppIds.length === 0) return;

  const existing = live.triggerAppIds ?? [];
  const merged = [...new Set([...existing, ...callAppIds])];
  if (merged.length !== existing.length) {
    listenerStore.getState().setTriggerAppIds(merged);
  }
}

export async function syncActiveMeetingMetadata({
  sessionId,
  inspections,
  micApps,
  sessionTitle,
  selfNames,
  existingParticipantNames,
}: {
  sessionId: string;
  inspections: readonly MeetingAccessibilityInspection[];
  micApps: readonly MicApp[];
  sessionTitle: string;
  selfNames: readonly string[];
  existingParticipantNames: readonly string[];
}): Promise<{ addedParticipants: string[]; updatedTitle: string | null }> {
  reconcileRecordingTriggerApps(sessionId, micApps, inspections);

  const micAppIds = new Set(micApps.map((app) => app.id));
  const activeInspections = inspections.filter(
    (inspection) =>
      inspectionShowsActiveMeeting(inspection) &&
      (micAppIds.size === 0 || micAppIds.has(inspection.app.id)),
  );
  if (activeInspections.length === 0) {
    return { addedParticipants: [], updatedTitle: null };
  }

  const state = getSessionSyncState(sessionId);
  let updatedTitle: string | null = null;

  if (
    !state.appliedTitle &&
    isGenericSessionTitle(sessionTitle) &&
    !hasLiveSessionTitleDraft(sessionId)
  ) {
    const detectedTitle = activeInspections
      .map((inspection) => inspection.meetingTitle?.trim() ?? "")
      .find((title) => title && !isGenericSessionTitle(title));
    if (detectedTitle) {
      state.appliedTitle = true;
      try {
        await updateSession(sessionId, { title: detectedTitle });
        updatedTitle = detectedTitle;
      } catch (error) {
        state.appliedTitle = false;
        console.warn("[meeting-metadata] failed to set detected title", error);
      }
    }
  }

  const detectedNames = activeInspections.flatMap((inspection) => [
    ...(inspection.participants ?? []),
    ...(inspection.activeSpeaker ? [inspection.activeSpeaker] : []),
  ]);
  const newNames = filterNewParticipantNames({
    detectedNames,
    selfNames,
    existingParticipantNames,
    attemptedNames: state.attemptedParticipants,
  });

  const addedParticipants: string[] = [];
  for (const name of newNames) {
    const key = name.toLowerCase();
    state.attemptedParticipants.add(key);
    try {
      const existingHumans = await liveQueryClient.execute<{ id: string }>(
        "SELECT id FROM humans WHERE deleted_at IS NULL AND LOWER(TRIM(name)) = LOWER(?) LIMIT 1",
        [name],
      );
      const humanId =
        existingHumans[0]?.id ?? (await createHuman({ name }));
      await addSessionParticipant(sessionId, humanId, "meeting_ax");
      addedParticipants.push(name);
    } catch (error) {
      state.attemptedParticipants.delete(key);
      console.warn(
        "[meeting-metadata] failed to attach detected participant",
        name,
        error,
      );
    }
  }

  return { addedParticipants, updatedTitle };
}
