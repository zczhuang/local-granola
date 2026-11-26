import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import {
  commands as exportCommands,
  type ZoomSyncConfig,
  type ZoomSyncStatus,
} from "@anlg/plugin-export";
import { commands as openerCommands } from "@anlg/plugin-opener2";
import {
  ArrowsClockwise,
  ArrowSquareOut,
  CheckCircle,
  CircleNotch,
  Copy,
  VideoCamera,
  Warning,
} from "@anlg/ui/components/icons";
import { Button } from "@anlg/ui/components/ui/button";
import { Input } from "@anlg/ui/components/ui/input";
import { Switch } from "@anlg/ui/components/ui/switch";
import { toast } from "@anlg/ui/components/ui/toast";
import { cn, formatDistanceToNow } from "@anlg/utils";

export const ZOOM_SYNC_STATUS_QUERY_KEY = ["zoom-sync-status"] as const;

async function fetchZoomStatus(): Promise<ZoomSyncStatus> {
  const result = await exportCommands.getZoomSyncStatus();
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

export function ZoomSyncSettings() {
  const queryClient = useQueryClient();
  const statusQuery = useQuery({
    queryKey: ZOOM_SYNC_STATUS_QUERY_KEY,
    queryFn: fetchZoomStatus,
  });
  const status = statusQuery.data;

  const [showCredentialsForm, setShowCredentialsForm] = useState(false);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");

  const setCachedStatus = (next: ZoomSyncStatus) => {
    queryClient.setQueryData(ZOOM_SYNC_STATUS_QUERY_KEY, next);
  };

  const saveCredentialsMutation = useMutation({
    mutationFn: async () => {
      const result = await exportCommands.saveZoomCredentials(
        clientId.trim(),
        clientSecret.trim() || null,
      );
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (next) => {
      setCachedStatus(next);
      setClientId("");
      setClientSecret("");
      setShowCredentialsForm(false);
      toast.success("Zoom OAuth credentials saved");
    },
    onError: (err) => {
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const connectMutation = useMutation({
    mutationFn: async () => {
      const result = await exportCommands.connectZoom();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (next) => {
      setCachedStatus(next);
      toast.success(
        next.email ? `Connected Zoom (${next.email})` : "Connected Zoom",
      );
      // Trigger an initial Zoom meeting sync immediately after connecting.
      syncNowMutation.mutate();
    },
    onError: (err) => {
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const disconnectMutation = useMutation({
    mutationFn: async () => {
      const result = await exportCommands.disconnectZoom();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (next) => {
      setCachedStatus(next);
      toast.success("Disconnected Zoom");
    },
    onError: (err) => {
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const updateConfigMutation = useMutation({
    mutationFn: async (patch: Partial<ZoomSyncConfig>) => {
      const current = status?.config ?? {
        autoSync: true,
        syncCalendarEvents: true,
        enrichSessions: true,
      };
      const result = await exportCommands.updateZoomSyncConfig({
        ...current,
        ...patch,
      });
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
      const result = await exportCommands.syncZoomMeetings(null);
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
    onSuccess: (report) => {
      void queryClient.invalidateQueries({
        queryKey: ZOOM_SYNC_STATUS_QUERY_KEY,
      });
      const parts: string[] = [];
      parts.push(`${report.meetingsSynced} meetings`);
      if (report.participantsAdded > 0) {
        parts.push(`${report.participantsAdded} participants added`);
      }
      if (report.sessionsEnriched > 0) {
        parts.push(`${report.sessionsEnriched} notes enriched`);
      }
      toast.success(`Zoom synced (${parts.join(", ")})`);
    },
    onError: (err) => {
      toast.error(err instanceof Error ? err.message : String(err));
    },
  });

  const handleCopyRedirectUri = async () => {
    const uri = status?.redirectUri ?? "http://127.0.0.1:43821/zoom/callback";
    try {
      await navigator.clipboard.writeText(uri);
      toast.success("Redirect URL copied");
    } catch {
      toast.error("Could not copy Redirect URL");
    }
  };

  if (statusQuery.isLoading) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <CircleNotch className="size-4 animate-spin" />
        Loading Zoom sync status…
      </div>
    );
  }

  const configured = status?.configured ?? false;
  const connected = status?.connected ?? false;
  const redirectUri =
    status?.redirectUri ?? "http://127.0.0.1:43821/zoom/callback";

  return (
    <section className="flex flex-col gap-5 border-t border-border/60 pt-8">
      <div>
        <h2 className="font-sans text-lg font-semibold tracking-tight">
          Zoom (Direct Local OAuth)
        </h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Pull scheduled Zoom meeting topics, join links, invitees, and
          registrant/participant emails directly from Zoom — stored locally in
          Oatmeal with no cloud relay.
        </p>
      </div>

      {/* Connection card */}
      <div className="rounded-xl border border-border/70 bg-card/50 p-4">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="flex min-w-0 items-start gap-3">
            <div
              className={cn(
                "flex size-9 shrink-0 items-center justify-center rounded-lg",
                connected
                  ? "bg-blue-500/10 text-blue-600 dark:text-blue-400"
                  : "bg-muted text-muted-foreground",
              )}
            >
              {connected ? (
                status?.lastError ? (
                  <Warning className="size-4 text-amber-500" />
                ) : (
                  <CheckCircle className="size-4" />
                )
              ) : (
                <VideoCamera className="size-4" />
              )}
            </div>
            <div className="min-w-0">
              <p className="text-sm font-medium">
                {connected
                  ? `Connected to ${status?.email ?? "Zoom"}`
                  : configured
                    ? "Zoom OAuth ready to connect"
                    : "Zoom OAuth not configured"}
              </p>
              <p className="mt-0.5 text-xs text-muted-foreground">
                {connected
                  ? status?.lastSyncedAtMs
                    ? `${status.syncedMeetingsCount} Zoom meeting${status.syncedMeetingsCount === 1 ? "" : "s"} synced · Last synced ${formatDistanceToNow(new Date(status.lastSyncedAtMs), { addSuffix: true })}`
                    : "Connected — click Sync Zoom now to pull meetings and participant emails."
                  : configured
                    ? `Using local Zoom OAuth app (${status?.clientIdHint ?? ""}).`
                    : "Create a free Zoom Marketplace General App and paste your Client ID & Secret below."}
              </p>
              {status?.lastError ? (
                <p className="mt-2 break-words text-xs text-amber-600 dark:text-amber-400">
                  {status.lastError}
                </p>
              ) : null}
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {connected ? (
              <>
                <Button
                  size="sm"
                  onClick={() => syncNowMutation.mutate()}
                  disabled={syncNowMutation.isPending}
                >
                  {syncNowMutation.isPending ? (
                    <CircleNotch className="size-3.5 animate-spin" />
                  ) : (
                    <ArrowsClockwise className="size-3.5" />
                  )}
                  Sync Zoom now
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => disconnectMutation.mutate()}
                  disabled={disconnectMutation.isPending}
                >
                  Disconnect
                </Button>
              </>
            ) : (
              <Button
                size="sm"
                onClick={() => connectMutation.mutate()}
                disabled={!configured || connectMutation.isPending}
              >
                {connectMutation.isPending ? (
                  <CircleNotch className="size-3.5 animate-spin" />
                ) : (
                  <VideoCamera className="size-3.5" />
                )}
                Connect Zoom
              </Button>
            )}
          </div>
        </div>

        {/* Credentials disclosure */}
        <div className="mt-4 border-t border-border/50 pt-3">
          <div className="flex items-center justify-between gap-2">
            <span className="text-xs text-muted-foreground">
              {configured
                ? `Local Zoom OAuth Client ID: ${status?.clientIdHint ?? "saved"}`
                : "Zoom OAuth credentials required"}
            </span>
            <Button
              size="sm"
              variant="ghost"
              className="h-7 px-2 text-xs"
              onClick={() => setShowCredentialsForm((prev) => !prev)}
            >
              {showCredentialsForm
                ? "Hide credentials"
                : configured
                  ? "Update credentials"
                  : "Set up Zoom OAuth"}
            </Button>
          </div>

          {(showCredentialsForm || !configured) && (
            <div className="mt-3 flex flex-col gap-3 rounded-lg bg-muted/40 p-3 text-xs">
              <div className="flex flex-col gap-1.5 text-muted-foreground">
                <p className="font-medium text-foreground">
                  Quick setup (Zoom Marketplace General App)
                </p>
                <ol className="list-inside list-decimal space-y-1">
                  <li>
                    Open{" "}
                    <button
                      type="button"
                      className="inline-flex items-center gap-0.5 text-foreground underline underline-offset-2"
                      onClick={() =>
                        void openerCommands.openUrl(
                          "https://marketplace.zoom.us/develop/create",
                          null,
                        )
                      }
                    >
                      Zoom App Marketplace
                      <ArrowSquareOut className="size-3" />
                    </button>{" "}
                    and create a <strong>General App</strong> (User-managed).
                  </li>
                  <li>
                    Add this exact <strong>OAuth Redirect URL</strong>:
                    <span className="mt-1 flex items-center gap-1.5">
                      <code className="rounded bg-background px-1.5 py-0.5 font-mono text-[11px] text-foreground">
                        {redirectUri}
                      </code>
                      <Button
                        type="button"
                        size="sm"
                        variant="outline"
                        className="h-6 px-2 text-[11px]"
                        onClick={() => void handleCopyRedirectUri()}
                      >
                        <Copy className="size-3" />
                        Copy
                      </Button>
                    </span>
                  </li>
                  <li>
                    Add scopes:{" "}
                    <code className="rounded bg-background px-1 py-0.5 font-mono text-[11px]">
                      user:read:user
                    </code>
                    ,{" "}
                    <code className="rounded bg-background px-1 py-0.5 font-mono text-[11px]">
                      meeting:read:list_meetings
                    </code>
                    ,{" "}
                    <code className="rounded bg-background px-1 py-0.5 font-mono text-[11px]">
                      meeting:read:meeting
                    </code>
                    ,{" "}
                    <code className="rounded bg-background px-1 py-0.5 font-mono text-[11px]">
                      meeting:read:list_registrants
                    </code>{" "}
                    (and optionally{" "}
                    <code className="rounded bg-background px-1 py-0.5 font-mono text-[11px]">
                      meeting:read:list_past_participants
                    </code>
                    ).
                  </li>
                </ol>
              </div>

              <div className="grid gap-2 sm:grid-cols-2">
                <Input
                  placeholder="Zoom Client ID"
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                  className="h-8 text-xs"
                />
                <Input
                  type="password"
                  placeholder="Zoom Client Secret"
                  value={clientSecret}
                  onChange={(e) => setClientSecret(e.target.value)}
                  className="h-8 text-xs"
                />
              </div>
              <div className="flex justify-end">
                <Button
                  size="sm"
                  disabled={
                    !clientId.trim() || saveCredentialsMutation.isPending
                  }
                  onClick={() => saveCredentialsMutation.mutate()}
                >
                  {saveCredentialsMutation.isPending ? (
                    <CircleNotch className="size-3.5 animate-spin" />
                  ) : null}
                  Save Zoom credentials
                </Button>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Sync preferences */}
      {status && (
        <div className="flex flex-col gap-3 rounded-xl border border-border/60 p-4">
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-sm font-medium">
                Sync Zoom meetings to Calendar
              </p>
              <p className="text-xs text-muted-foreground">
                Add scheduled Zoom meetings with join links and registrants to
                Oatmeal’s calendar for reminders and auto-start.
              </p>
            </div>
            <Switch
              aria-label="Sync Zoom meetings to Calendar"
              checked={status.config.syncCalendarEvents}
              onCheckedChange={(syncCalendarEvents) =>
                updateConfigMutation.mutate({ syncCalendarEvents })
              }
            />
          </div>

          <div className="flex items-center justify-between gap-4 border-t border-border/50 pt-3">
            <div>
              <p className="text-sm font-medium">
                Enrich notes &amp; participant emails
              </p>
              <p className="text-xs text-muted-foreground">
                Match recordings to Zoom meetings to fill titles and attach
                registrant/participant names and emails automatically.
              </p>
            </div>
            <Switch
              aria-label="Enrich notes and participant emails"
              checked={status.config.enrichSessions}
              onCheckedChange={(enrichSessions) =>
                updateConfigMutation.mutate({ enrichSessions })
              }
            />
          </div>

          <div className="flex items-center justify-between gap-4 border-t border-border/50 pt-3">
            <div>
              <p className="text-sm font-medium">Auto-sync Zoom in background</p>
              <p className="text-xs text-muted-foreground">
                Refresh upcoming Zoom meetings periodically and enrich notes
                right after calls finish.
              </p>
            </div>
            <Switch
              aria-label="Auto-sync Zoom in background"
              checked={status.config.autoSync}
              onCheckedChange={(autoSync) =>
                updateConfigMutation.mutate({ autoSync })
              }
            />
          </div>
        </div>
      )}

      {/* Upcoming Zoom meetings preview */}
      {connected && status && status.upcomingMeetings.length > 0 && (
        <div className="flex flex-col gap-2 rounded-xl border border-border/60 p-4">
          <p className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
            Synced Upcoming Zoom Meetings
          </p>
          <div className="divide-y divide-border/50">
            {status.upcomingMeetings.map((m) => (
              <div
                key={`${m.meetingId}-${m.startTime ?? ""}`}
                className="flex items-center justify-between gap-3 py-2 text-sm"
              >
                <div className="min-w-0">
                  <p className="truncate font-medium">{m.topic}</p>
                  <p className="text-xs text-muted-foreground">
                    {m.startTime
                      ? new Date(m.startTime).toLocaleString()
                      : "Scheduled"}{" "}
                    · {m.participantCount} invitee
                    {m.participantCount === 1 ? "" : "s"}
                  </p>
                </div>
                {m.joinUrl && (
                  <Button
                    size="sm"
                    variant="outline"
                    className="h-7 shrink-0 px-2.5 text-xs"
                    onClick={() =>
                      void openerCommands.openUrl(m.joinUrl!, null)
                    }
                  >
                    Join
                  </Button>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
