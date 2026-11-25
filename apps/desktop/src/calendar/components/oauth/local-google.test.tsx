import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getLocalGoogleCalendarStatus: vi.fn(),
  saveLocalGoogleCalendarCredentials: vi.fn(),
  connectLocalGoogleCalendar: vi.fn(),
  disconnectLocalGoogleCalendar: vi.fn(),
  scheduleSync: vi.fn(),
  allowReconnectedCalendarConnections: vi.fn(),
  removeDisconnectedCalendarConnection: vi.fn(),
}));

vi.mock("@anlg/plugin-calendar", () => ({
  commands: {
    getLocalGoogleCalendarStatus: mocks.getLocalGoogleCalendarStatus,
    saveLocalGoogleCalendarCredentials:
      mocks.saveLocalGoogleCalendarCredentials,
    connectLocalGoogleCalendar: mocks.connectLocalGoogleCalendar,
    disconnectLocalGoogleCalendar: mocks.disconnectLocalGoogleCalendar,
  },
}));

vi.mock("@iconify-icon/react", () => ({
  Icon: (props: Record<string, unknown>) =>
    createElement("iconify-icon", props),
}));

vi.mock("../context", () => ({
  useSync: () => ({
    cancelDebouncedSync: vi.fn(),
    status: "idle",
    scheduleDebouncedSync: vi.fn(),
    scheduleSync: mocks.scheduleSync,
  }),
}));

vi.mock("~/calendar/queries", () => ({
  setCalendarEnabled: vi.fn(),
  useCalendarRows: () => [],
}));

vi.mock("~/services/calendar", () => ({
  allowReconnectedCalendarConnections:
    mocks.allowReconnectedCalendarConnections,
  removeDisconnectedCalendarConnection:
    mocks.removeDisconnectedCalendarConnection,
}));

import { PROVIDERS } from "../shared";
import { LocalGoogleProviderContent } from "./local-google";

const GOOGLE_PROVIDER = PROVIDERS.find((provider) => provider.id === "google")!;

function renderWithQueryClient(ui: ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}

describe("LocalGoogleProviderContent", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("prompts for Google Cloud Desktop OAuth credentials when unconfigured and saves them", async () => {
    mocks.getLocalGoogleCalendarStatus.mockResolvedValue({
      status: "ok",
      data: { configured: false, client_id_preview: null, accounts: [] },
    });
    mocks.saveLocalGoogleCalendarCredentials.mockResolvedValue({
      status: "ok",
      data: {
        configured: true,
        client_id_preview: "12345…leusercontent.com",
        accounts: [],
      },
    });

    renderWithQueryClient(
      <LocalGoogleProviderContent config={GOOGLE_PROVIDER} connections={[]} />,
    );

    const textarea = await screen.findByPlaceholderText(
      /Paste client_secret_\*\.json/i,
    );
    fireEvent.change(textarea, {
      target: {
        value:
          '{"installed":{"client_id":"123.apps.googleusercontent.com","client_secret":"sec"}}',
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save credentials" }));

    await waitFor(() => {
      expect(mocks.saveLocalGoogleCalendarCredentials).toHaveBeenCalledWith(
        '{"installed":{"client_id":"123.apps.googleusercontent.com","client_secret":"sec"}}',
        null,
      );
    });
  });

  it("connects a local Google account and triggers calendar sync when configured", async () => {
    mocks.getLocalGoogleCalendarStatus.mockResolvedValue({
      status: "ok",
      data: {
        configured: true,
        client_id_preview: "12345…leusercontent.com",
        accounts: [],
      },
    });
    mocks.connectLocalGoogleCalendar.mockResolvedValue({
      status: "ok",
      data: {
        connection_id: "local-google:alex@example.com",
        email: "alex@example.com",
      },
    });

    renderWithQueryClient(
      <LocalGoogleProviderContent config={GOOGLE_PROVIDER} connections={[]} />,
    );

    const connectBtn = await screen.findByRole("button", {
      name: "Connect Google Calendar",
    });
    fireEvent.click(connectBtn);

    await waitFor(() => {
      expect(mocks.allowReconnectedCalendarConnections).toHaveBeenCalledWith(
        "google-calendar",
      );
      expect(mocks.connectLocalGoogleCalendar).toHaveBeenCalled();
      expect(mocks.scheduleSync).toHaveBeenCalled();
    });
  });
});
