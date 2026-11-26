import { useEffect, useRef } from "react";

import { commands as exportCommands } from "@anlg/plugin-export";

import { listenerStore } from "~/store/zustand/listener/instance";

const STARTUP_SYNC_DELAY_MS = 10_000;
const POST_MEETING_SYNC_DELAY_MS = 8_000;
const PERIODIC_SYNC_INTERVAL_MS = 15 * 60_000;

async function syncZoomIfEnabled(sessionId: string | null) {
  try {
    const status = await exportCommands.getZoomSyncStatus();
    if (
      status.status !== "ok" ||
      !status.data.connected ||
      !status.data.config.autoSync
    ) {
      return;
    }
    await exportCommands.syncZoomMeetings(sessionId);
  } catch (error) {
    console.warn("[zoom-auto-sync] background sync failed", error);
  }
}

export function ZoomAutoSyncLifecycle() {
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const scheduleSync = (sessionId: string | null, delayMs: number) => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
      }
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        void syncZoomIfEnabled(sessionId);
      }, delayMs);
    };

    const startupTimer = setTimeout(() => {
      void syncZoomIfEnabled(null);
    }, STARTUP_SYNC_DELAY_MS);

    let prevStatus = listenerStore.getState().live.status;
    let prevSessionId = listenerStore.getState().live.sessionId;
    const unsubListener = listenerStore.subscribe((state) => {
      const nextStatus = state.live.status;
      const nextSessionId = state.live.sessionId;
      if (
        (prevStatus === "active" || prevStatus === "finalizing") &&
        nextStatus === "inactive" &&
        prevSessionId &&
        state.live.pausedSessionId !== prevSessionId
      ) {
        scheduleSync(prevSessionId, POST_MEETING_SYNC_DELAY_MS);
      }
      prevStatus = nextStatus;
      prevSessionId = nextSessionId;
    });

    const interval = setInterval(() => {
      if (listenerStore.getState().live.status === "inactive") {
        void syncZoomIfEnabled(null);
      }
    }, PERIODIC_SYNC_INTERVAL_MS);

    return () => {
      clearTimeout(startupTimer);
      unsubListener();
      clearInterval(interval);
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, []);

  return null;
}
