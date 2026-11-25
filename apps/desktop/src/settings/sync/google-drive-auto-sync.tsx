import { useEffect, useRef } from "react";

import { commands as exportCommands } from "@anlg/plugin-export";

import { getEnhancerService } from "~/services/enhancer";
import { listenerStore } from "~/store/zustand/listener/instance";

const POST_MEETING_SYNC_DELAY_MS = 15_000;
const PERIODIC_SYNC_INTERVAL_MS = 5 * 60_000;

async function syncIfAutoSyncEnabled(sessionId: string | null) {
  try {
    const status = await exportCommands.getGoogleDriveSyncStatus();
    if (
      status.status !== "ok" ||
      !status.data.connected ||
      !status.data.config.autoSync
    ) {
      return;
    }
    await exportCommands.syncNotesToGoogleDrive(sessionId);
  } catch (error) {
    console.warn("[google-drive-auto-sync] background sync failed", error);
  }
}

export function GoogleDriveAutoSyncLifecycle() {
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const scheduleSync = (sessionId: string | null, delayMs: number) => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
      }
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        void syncIfAutoSyncEnabled(sessionId);
      }, delayMs);
    };

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

    const enhancer = getEnhancerService();
    const unsubEnhancer = enhancer?.on((event) => {
      if (event.type === "auto-enhance-started") {
        // Give local Ollama summary time to finish and persist before syncing.
        scheduleSync(event.sessionId, 30_000);
      }
    });

    const interval = setInterval(() => {
      if (listenerStore.getState().live.status === "inactive") {
        void syncIfAutoSyncEnabled(null);
      }
    }, PERIODIC_SYNC_INTERVAL_MS);

    return () => {
      unsubListener();
      unsubEnhancer?.();
      clearInterval(interval);
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, []);

  return null;
}
