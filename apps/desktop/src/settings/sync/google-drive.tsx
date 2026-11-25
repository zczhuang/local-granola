import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import {
  commands as exportCommands,
  type GoogleDriveSyncConfig,
  type GoogleDriveSyncFormat,
  type GoogleDriveSyncStatus,
} from "@anlg/plugin-export";
import { commands as openerCommands } from "@anlg/plugin-opener2";
import {
  ArrowsClockwise,
  ArrowSquareOut,
  CheckCircle,
  CircleNotch,
  CloudSlash,
  Warning,
} from "@anlg/ui/components/icons";
import { Button } from "@anlg/ui/components/ui/button";
import { Input } from "@anlg/ui/components/ui/input";
import { Switch } from "@anlg/ui/components/ui/switch";
import { toast } from "@anlg/ui/components/ui/toast";
import { cn, formatDistanceToNow } from "@anlg/utils";

import { SettingsPageTitle } from "~/settings/page-title";

export const GOOGLE_DRIVE_SYNC_STATUS_QUERY_KEY = [
  "google-drive-sync-status",
] as const;

async function fetchStatus(): Promise<GoogleDriveSyncStatus> {
  const result = await exportCommands.getGoogleDriveSyncStatus();
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

export function GoogleDriveSyncSettings() {
  const queryClient = useQueryClient();
  const statusQuery = useQuery({
    queryKey: GOOGLE_DRIVE_SYNC_STATUS_QUERY_KEY,
    queryFn: fetchStatus,
  });
  const status = statusQuery.data;

  const [showCredentialsForm, setShowCredentialsForm] = useState(false);
  const [clientInput, setClientInput] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [folderNameDraft, setFolderNameDraft] = useState("Oatmeal Notes");

  useEffect(() => {
    if (status?.config.folderName) {
      setFolderNameDraft(status.config.folderName);
    }
  }, [status?.config.folderName]);

  const setCachedStatus = (next: GoogleDriveSyncStatus) => {
    queryClient.setQueryData(GOOGLE_DRIVE_SYNC_STATUS_QUERY_KEY, next);
  };

  const saveCredentialsMutation = useMutation({
    mutationFn: async () => {
      const result = await exportCommands.saveGoogleDriveCredentials(
        clientInput.trim(),
        clientSecret.trim() || null,
      );
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (next) => {
      setCachedStatus(next);
      setClientInput("");
      setClientSecret("");
      setShowCredentialsForm(false);
      toast.success("Google OAuth credentials saved");
    },
    onError: (err) => {
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const connectMutation = useMutation({
    mutationFn: async () => {
      const result = await exportCommands.connectGoogleDrive();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (next) => {
      setCachedStatus(next);
      toast.success(
        next.email
          ? `Connected Google Drive (${next.email})`
          : "Connected Google Drive",
      );
    },
    onError: (err) => {
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const disconnectMutation = useMutation({
    mutationFn: async () => {
      const result = await exportCommands.disconnectGoogleDrive();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (next) => {
      setCachedStatus(next);
      toast.success("Disconnected Google Drive");
    },
    onError: (err) => {
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const updateConfigMutation = useMutation({
    mutationFn: async (nextConfig: GoogleDriveSyncConfig) => {
      const result = await exportCommands.updateGoogleDriveSyncConfig(nextConfig);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (next) => {
      setCachedStatus(next);
    },
    onError: (err) => {
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const syncNowMutation = useMutation({
    mutationFn: async () => {
      const result = await exportCommands.syncNotesToGoogleDrive(null);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (report) => {
      void queryClient.invalidateQueries({
        queryKey: GOOGLE_DRIVE_SYNC_STATUS_QUERY_KEY,
      });
      if (report.failed > 0) {
        toast.warning(
          `Synced ${report.uploaded + report.updated} notes, ${report.failed} failed`,
          { description: report.errors[0] },
        );
      } else if (report.uploaded === 0 && report.updated === 0) {
        toast.success("All notes are already up to date in Google Drive");
      } else {
        const parts: string[] = [];
        if (report.uploaded > 0) parts.push(`${report.uploaded} new`);
        if (report.updated > 0) parts.push(`${report.updated} updated`);
        toast.success(`Synced to Google Drive (${parts.join(", ")})`);
      }
    },
    onError: (err) => {
      void queryClient.invalidateQueries({
        queryKey: GOOGLE_DRIVE_SYNC_STATUS_QUERY_KEY,
      });
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const currentConfig: GoogleDriveSyncConfig = status?.config ?? {
    folderName: "Oatmeal Notes",
    format: "google_doc",
    includeTranscript: true,
    autoSync: true,
  };

  const applyConfigPatch = (patch: Partial<GoogleDriveSyncConfig>) => {
    updateConfigMutation.mutate({
      ...currentConfig,
      ...patch,
    });
  };

  const folderNameDirty =
    folderNameDraft.trim() !== "" &&
    folderNameDraft.trim() !== currentConfig.folderName;

  const isConfigured = status?.configured ?? false;
  const isConnected = status?.connected ?? false;
  const needsCredentials = !isConfigured || showCredentialsForm;

  return (
    <div className="flex max-w-2xl flex-col gap-8">
      <SettingsPageTitle title="Google Drive Sync" />

      <section className="border-border bg-background/60 flex flex-col gap-4 rounded-xl border p-4">
        <div className="flex items-start justify-between gap-4">
          <div className="flex min-w-0 gap-3">
            <div className="bg-accent/60 flex size-9 shrink-0 items-center justify-center rounded-full">
              {syncNowMutation.isPending ? (
                <ArrowsClockwise className="size-4 animate-spin text-blue-500" />
              ) : status?.lastError ? (
                <Warning className="size-4 text-amber-500" />
              ) : isConnected ? (
                <CheckCircle className="size-4 text-emerald-500" />
              ) : (
                <CloudSlash className="text-muted-foreground size-4" />
              )}
            </div>
            <div className="min-w-0">
              <h3 className="text-sm font-medium">
                {isConnected
                  ? `Connected to ${status?.email ?? "Google Drive"}`
                  : "Sync notes directly to your Google Drive"}
              </h3>
              <p className="text-muted-foreground mt-1 text-xs leading-5">
                {isConnected
                  ? status?.lastSyncedAtMs
                    ? `${status.syncedCount} synced ${status.syncedCount === 1 ? "note" : "notes"} · Last synced ${formatDistanceToNow(new Date(status.lastSyncedAtMs), { addSuffix: true })}`
                    : "Connected with drive.file scope — Oatmeal only accesses files it creates."
                  : "Uploads your meeting notes and summaries directly from your Mac to Google Drive with zero Oatmeal cloud relay. Uses the least-privilege drive.file scope."}
              </p>
              {status?.lastError ? (
                <p className="mt-1.5 text-xs leading-5 text-amber-600 dark:text-amber-400">
                  {status.lastError}
                </p>
              ) : null}
            </div>
          </div>

          <div className="flex shrink-0 items-center gap-2">
            {isConnected ? (
              <>
                <Button
                  type="button"
                  size="sm"
                  onClick={() => syncNowMutation.mutate()}
                  disabled={syncNowMutation.isPending}
                >
                  {syncNowMutation.isPending ? (
                    <CircleNotch className="mr-1.5 size-3.5 animate-spin" />
                  ) : (
                    <ArrowsClockwise className="mr-1.5 size-3.5" />
                  )}
                  Sync now
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => disconnectMutation.mutate()}
                  disabled={disconnectMutation.isPending}
                >
                  Disconnect
                </Button>
              </>
            ) : isConfigured ? (
              <Button
                type="button"
                size="sm"
                onClick={() => connectMutation.mutate()}
                disabled={connectMutation.isPending}
              >
                {connectMutation.isPending ? (
                  <CircleNotch className="mr-1.5 size-3.5 animate-spin" />
                ) : null}
                Connect Google Drive
              </Button>
            ) : null}
          </div>
        </div>

        {isConnected && status?.rootFolderUrl ? (
          <div className="border-border/60 flex items-center justify-between border-t pt-3 text-xs">
            <span className="text-muted-foreground">
              Destination folder:{" "}
              <span className="text-foreground font-medium">
                {currentConfig.folderName}
              </span>
            </span>
            <button
              type="button"
              onClick={() =>
                void openerCommands.openUrl(status.rootFolderUrl!, null)
              }
              className="text-foreground hover:text-foreground/80 inline-flex items-center gap-1 font-medium underline-offset-4 hover:underline"
            >
              Open in Google Drive
              <ArrowSquareOut className="size-3.5" />
            </button>
          </div>
        ) : null}

        {isConfigured && !showCredentialsForm ? (
          <div className="border-border/60 flex items-center justify-between border-t pt-3 text-xs">
            <span className="text-muted-foreground">
              {status?.credentialsSource === "calendar"
                ? `Reusing your Google OAuth client from Google Calendar (${status.clientIdHint ?? ""})`
                : `Google OAuth client configured (${status?.clientIdHint ?? ""})`}
            </span>
            <button
              type="button"
              onClick={() => setShowCredentialsForm(true)}
              className="text-muted-foreground hover:text-foreground underline-offset-4 hover:underline"
            >
              Change OAuth client
            </button>
          </div>
        ) : null}
      </section>

      {needsCredentials ? (
        <section className="border-border bg-background/60 flex flex-col gap-3 rounded-xl border p-4">
          <div className="flex items-center justify-between">
            <h4 className="text-sm font-medium">
              Google Cloud Desktop OAuth client
            </h4>
            {isConfigured ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => setShowCredentialsForm(false)}
              >
                Cancel
              </Button>
            ) : null}
          </div>
          <p className="text-muted-foreground text-xs leading-5">
            In Google Cloud Console, enable <strong>Google Drive API</strong>,
            create a <strong>Desktop app</strong> OAuth client, and paste its
            JSON file contents (or Client ID) below. Credentials stay on your
            Mac.
          </p>
          <Input
            type="text"
            value={clientInput}
            onChange={(event) => setClientInput(event.target.value)}
            placeholder="Paste client_secret_*.json or Client ID (…apps.googleusercontent.com)"
            aria-label="Google OAuth Client ID or JSON"
          />
          {clientInput.trim() && !clientInput.trim().startsWith("{") ? (
            <Input
              type="password"
              value={clientSecret}
              onChange={(event) => setClientSecret(event.target.value)}
              placeholder="Client Secret (optional if embedded in JSON)"
              aria-label="Google OAuth Client Secret"
            />
          ) : null}
          <div className="flex items-center justify-end gap-2">
            <Button
              type="button"
              size="sm"
              disabled={!clientInput.trim() || saveCredentialsMutation.isPending}
              onClick={() => saveCredentialsMutation.mutate()}
            >
              {saveCredentialsMutation.isPending
                ? "Saving..."
                : "Save credentials"}
            </Button>
          </div>
        </section>
      ) : null}

      <section className="flex flex-col gap-5">
        <h4 className="text-sm font-medium">Sync preferences</h4>

        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium">Drive folder name</p>
            <p className="text-muted-foreground text-xs">
              Oatmeal creates this folder in My Drive and mirrors your Oatmeal
              subfolders inside it.
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Input
              type="text"
              value={folderNameDraft}
              onChange={(event) => setFolderNameDraft(event.target.value)}
              aria-label="Drive folder name"
              className="w-44"
            />
            {folderNameDirty ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={updateConfigMutation.isPending}
                onClick={() =>
                  applyConfigPatch({ folderName: folderNameDraft.trim() })
                }
              >
                Save
              </Button>
            ) : null}
          </div>
        </div>

        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium">File format</p>
            <p className="text-muted-foreground text-xs">
              Google Docs lets you view, comment, and share directly in Drive;
              Markdown keeps raw <code>.md</code> files.
            </p>
          </div>
          <div
            role="group"
            aria-label="Google Drive file format"
            className="bg-accent/60 flex shrink-0 items-center gap-1 rounded-lg p-1"
          >
            {(
              [
                { value: "google_doc", label: "Google Docs" },
                { value: "markdown", label: "Markdown (.md)" },
              ] as Array<{ value: GoogleDriveSyncFormat; label: string }>
            ).map((option) => {
              const active = currentConfig.format === option.value;
              return (
                <button
                  key={option.value}
                  type="button"
                  aria-pressed={active}
                  onClick={() => applyConfigPatch({ format: option.value })}
                  className={cn([
                    "rounded-md px-2.5 py-1 text-xs font-medium transition-colors",
                    active
                      ? "bg-background text-foreground shadow-xs"
                      : "text-muted-foreground hover:text-foreground",
                  ])}
                >
                  {option.label}
                </button>
              );
            })}
          </div>
        </div>

        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium">Include transcript</p>
            <p className="text-muted-foreground text-xs">
              Append the full speaker-labeled transcript below your notes and
              summary.
            </p>
          </div>
          <Switch
            aria-label="Include transcript"
            checked={currentConfig.includeTranscript}
            onCheckedChange={(checked) =>
              applyConfigPatch({ includeTranscript: checked })
            }
          />
        </div>

        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium">Auto-sync meetings</p>
            <p className="text-muted-foreground text-xs">
              Automatically sync notes to Google Drive after meetings finish and
              on periodic background checks.
            </p>
          </div>
          <Switch
            aria-label="Auto-sync meetings"
            checked={currentConfig.autoSync}
            onCheckedChange={(checked) =>
              applyConfigPatch({ autoSync: checked })
            }
          />
        </div>
      </section>
    </div>
  );
}
