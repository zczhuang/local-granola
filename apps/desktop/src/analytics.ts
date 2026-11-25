import {
  commands as analyticsCommands,
  type JsonValue,
} from "@anlg/plugin-analytics";

import { LOCAL_ONLY } from "~/shared/local-only";

export function trackAnalyticsEvent(
  event: string,
  properties: Record<string, JsonValue> = {},
) {
  if (LOCAL_ONLY) return;
  try {
    const capture = analyticsCommands.eventFireAndForget;
    if (typeof capture !== "function") return;

    const pending = capture({
      event,
      ...properties,
    });
    void pending.catch((error: unknown) => {
      console.warn(`[analytics] failed to record ${event}`, error);
    });
  } catch (error) {
    console.warn(`[analytics] failed to record ${event}`, error);
  }
}
