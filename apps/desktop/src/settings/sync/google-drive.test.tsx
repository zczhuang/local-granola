import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getGoogleDriveSyncStatus: vi.fn(),
  saveGoogleDriveCredentials: vi.fn(),
  connectGoogleDrive: vi.fn(),
  disconnectGoogleDrive: vi.fn(),
  updateGoogleDriveSyncConfig: vi.fn(),
  syncNotesToGoogleDrive: vi.fn(),
  openUrl: vi.fn(),
}));

vi.mock("@anlg/plugin-export", () => ({
  commands: {
    getGoogleDriveSyncStatus: mocks.getGoogleDriveSyncStatus,
    saveGoogleDriveCredentials: mocks.saveGoogleDriveCredentials,
    connectGoogleDrive: mocks.connectGoogleDrive,
    disconnectGoogleDrive: mocks.disconnectGoogleDrive,
    updateGoogleDriveSyncConfig: mocks.updateGoogleDriveSyncConfig,
    syncNotesToGoogleDrive: mocks.syncNotesToGoogleDrive,
  },
}));

vi.mock("@anlg/plugin-opener2", () => ({
  commands: { openUrl: mocks.openUrl },
}));

vi.mock("@anlg/ui/components/ui/toast", () => ({
  toast: {
    success: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
  },
}));

import { GoogleDriveSyncSettings } from "./google-drive";

function renderWithQuery() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <GoogleDriveSyncSettings />
    </QueryClientProvider>,
  );
}

describe("GoogleDriveSyncSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    cleanup();
  });

  it("reuses Google Calendar OAuth credentials and lets user connect Google Drive in one click", async () => {
    mocks.getGoogleDriveSyncStatus.mockResolvedValue({
      status: "ok",
      data: {
        configured: true,
        credentialsSource: "calendar",
        clientIdHint: "1234567890…nt.com",
        connected: false,
        email: null,
        rootFolderId: null,
        rootFolderUrl: null,
        syncedCount: 0,
        lastSyncedAtMs: null,
        lastError: null,
        config: {
          folderName: "Oatmeal Notes",
          format: "google_doc",
          includeTranscript: true,
          autoSync: true,
        },
      },
    });
    mocks.connectGoogleDrive.mockResolvedValue({
      status: "ok",
      data: {
        configured: true,
        credentialsSource: "calendar",
        clientIdHint: "1234567890…nt.com",
        connected: true,
        email: "alex@example.com",
        rootFolderId: null,
        rootFolderUrl: null,
        syncedCount: 0,
        lastSyncedAtMs: null,
        lastError: null,
        config: {
          folderName: "Oatmeal Notes",
          format: "google_doc",
          includeTranscript: true,
          autoSync: true,
        },
      },
    });

    renderWithQuery();

    const connectBtn = await screen.findByRole("button", {
      name: "Connect Google Drive",
    });
    fireEvent.click(connectBtn);

    await waitFor(() => {
      expect(mocks.connectGoogleDrive).toHaveBeenCalledOnce();
      expect(screen.getByText("Connected to alex@example.com")).toBeTruthy();
    });
  });

  it("runs Sync now and updates format preferences", async () => {
    mocks.getGoogleDriveSyncStatus.mockResolvedValue({
      status: "ok",
      data: {
        configured: true,
        credentialsSource: "drive",
        clientIdHint: "1234567890…nt.com",
        connected: true,
        email: "alex@example.com",
        rootFolderId: "folder-1",
        rootFolderUrl: "https://drive.google.com/drive/folders/folder-1",
        syncedCount: 2,
        lastSyncedAtMs: Date.now() - 60_000,
        lastError: null,
        config: {
          folderName: "Oatmeal Notes",
          format: "google_doc",
          includeTranscript: true,
          autoSync: true,
        },
      },
    });
    mocks.syncNotesToGoogleDrive.mockResolvedValue({
      status: "ok",
      data: {
        uploaded: 1,
        updated: 0,
        skipped: 2,
        failed: 0,
        totalSynced: 3,
        folderUrl: "https://drive.google.com/drive/folders/folder-1",
        syncedAtMs: Date.now(),
        errors: [],
      },
    });
    mocks.updateGoogleDriveSyncConfig.mockResolvedValue({
      status: "ok",
      data: {
        configured: true,
        credentialsSource: "drive",
        clientIdHint: "1234567890…nt.com",
        connected: true,
        email: "alex@example.com",
        rootFolderId: "folder-1",
        rootFolderUrl: "https://drive.google.com/drive/folders/folder-1",
        syncedCount: 2,
        lastSyncedAtMs: Date.now() - 60_000,
        lastError: null,
        config: {
          folderName: "Oatmeal Notes",
          format: "markdown",
          includeTranscript: true,
          autoSync: true,
        },
      },
    });

    renderWithQuery();

    const syncBtn = await screen.findByRole("button", { name: "Sync now" });
    fireEvent.click(syncBtn);
    await waitFor(() => {
      expect(mocks.syncNotesToGoogleDrive).toHaveBeenCalledWith(null);
    });

    fireEvent.click(screen.getByRole("button", { name: "Markdown (.md)" }));
    await waitFor(() => {
      expect(mocks.updateGoogleDriveSyncConfig).toHaveBeenCalledWith({
        folderName: "Oatmeal Notes",
        format: "markdown",
        includeTranscript: true,
        autoSync: true,
      });
    });
  });
});
