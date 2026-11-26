import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getZoomSyncStatus: vi.fn(),
  saveZoomCredentials: vi.fn(),
  connectZoom: vi.fn(),
  disconnectZoom: vi.fn(),
  updateZoomSyncConfig: vi.fn(),
  syncZoomMeetings: vi.fn(),
}));

vi.mock("@anlg/plugin-export", () => ({
  commands: mocks,
}));

vi.mock("@anlg/plugin-opener2", () => ({
  commands: { openUrl: vi.fn() },
}));

vi.mock("@anlg/ui/components/ui/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

import { ZoomSyncSettings } from "./zoom";

function renderZoomSettings() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ZoomSyncSettings />
    </QueryClientProvider>,
  );
}

describe("ZoomSyncSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows setup guide and saves Zoom OAuth credentials", async () => {
    mocks.getZoomSyncStatus.mockResolvedValue({
      status: "ok",
      data: {
        configured: false,
        clientIdHint: null,
        redirectUri: "http://127.0.0.1:43821/zoom/callback",
        connected: false,
        email: null,
        displayName: null,
        syncedMeetingsCount: 0,
        enrichedSessionsCount: 0,
        lastSyncedAtMs: null,
        lastError: null,
        config: {
          autoSync: true,
          syncCalendarEvents: true,
          enrichSessions: true,
        },
        upcomingMeetings: [],
      },
    });
    mocks.saveZoomCredentials.mockResolvedValue({
      status: "ok",
      data: {
        configured: true,
        clientIdHint: "zoom12…7890",
        redirectUri: "http://127.0.0.1:43821/zoom/callback",
        connected: false,
        email: null,
        displayName: null,
        syncedMeetingsCount: 0,
        enrichedSessionsCount: 0,
        lastSyncedAtMs: null,
        lastError: null,
        config: {
          autoSync: true,
          syncCalendarEvents: true,
          enrichSessions: true,
        },
        upcomingMeetings: [],
      },
    });

    renderZoomSettings();

    expect(
      await screen.findByText("Zoom OAuth not configured"),
    ).toBeTruthy();
    expect(
      screen.getByText("http://127.0.0.1:43821/zoom/callback"),
    ).toBeTruthy();

    fireEvent.change(screen.getByPlaceholderText("Zoom Client ID"), {
      target: { value: "zoom1234567890" },
    });
    fireEvent.change(screen.getByPlaceholderText("Zoom Client Secret"), {
      target: { value: "secret-abc" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Save Zoom credentials" }),
    );

    await waitFor(() => {
      expect(mocks.saveZoomCredentials).toHaveBeenCalledWith(
        "zoom1234567890",
        "secret-abc",
      );
    });
  });

  it("shows connected account and syncs Zoom meetings on demand", async () => {
    mocks.getZoomSyncStatus.mockResolvedValue({
      status: "ok",
      data: {
        configured: true,
        clientIdHint: "zoom12…7890",
        redirectUri: "http://127.0.0.1:43821/zoom/callback",
        connected: true,
        email: "stuart@example.com",
        displayName: "Stuart Chuang",
        syncedMeetingsCount: 2,
        enrichedSessionsCount: 1,
        lastSyncedAtMs: Date.now() - 60_000,
        lastError: null,
        config: {
          autoSync: true,
          syncCalendarEvents: true,
          enrichSessions: true,
        },
        upcomingMeetings: [
          {
            meetingId: "99911122233",
            topic: "Weekly Engineering Sync",
            startTime: "2026-11-26T15:00:00Z",
            joinUrl: "https://us02web.zoom.us/j/99911122233",
            participantCount: 3,
          },
        ],
      },
    });
    mocks.syncZoomMeetings.mockResolvedValue({
      status: "ok",
      data: {
        meetingsSynced: 2,
        eventsUpserted: 2,
        sessionsEnriched: 1,
        participantsAdded: 3,
        syncedAtMs: Date.now(),
        errors: [],
      },
    });

    renderZoomSettings();

    expect(
      await screen.findByText("Connected to stuart@example.com"),
    ).toBeTruthy();
    expect(screen.getByText("Weekly Engineering Sync")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Sync Zoom now" }));
    await waitFor(() => {
      expect(mocks.syncZoomMeetings).toHaveBeenCalledWith(null);
    });
  });
});
