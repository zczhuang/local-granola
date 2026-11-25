import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo, useState } from "react";

import type { ConnectionItem } from "@anlg/api-client";
import { commands as calendarCommands } from "@anlg/plugin-calendar";
import { CircleNotch } from "@anlg/ui/components/icons";

import { useSync } from "../context";
import {
  OAuthCalendarSelection,
  useOAuthCalendarSelection,
} from "./calendar-selection";

import type { CalendarProvider } from "~/calendar/components/shared";
import {
  allowReconnectedCalendarConnections,
  removeDisconnectedCalendarConnection,
} from "~/services/calendar";

const LOCAL_GOOGLE_STATUS_QUERY_KEY = ["local-google-calendar-status"] as const;

export function LocalGoogleProviderContent({
  config,
  connections,
}: {
  config: CalendarProvider;
  connections: ConnectionItem[];
}) {
  const queryClient = useQueryClient();
  const { scheduleSync } = useSync();
  const statusQuery = useQuery({
    queryKey: LOCAL_GOOGLE_STATUS_QUERY_KEY,
    queryFn: async () => {
      const result = await calendarCommands.getLocalGoogleCalendarStatus();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      return result.data;
    },
  });

  const [editingCredentials, setEditingCredentials] = useState(false);
  const [credentialsInput, setCredentialsInput] = useState("");
  const [clientSecretInput, setClientSecretInput] = useState("");
  const [isSaving, setIsSaving] = useState(false);
  const [isConnecting, setIsConnecting] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const configured = statusQuery.data?.configured ?? false;
  const showSetupForm = !configured || editingCredentials;

  const refreshAll = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: LOCAL_GOOGLE_STATUS_QUERY_KEY }),
      queryClient.invalidateQueries({ queryKey: ["integration-status"] }),
    ]);
    scheduleSync();
  }, [queryClient, scheduleSync]);

  const handleSaveCredentials = useCallback(async () => {
    setErrorMessage(null);
    setIsSaving(true);
    try {
      const result = await calendarCommands.saveLocalGoogleCalendarCredentials(
        credentialsInput,
        clientSecretInput.trim() ? clientSecretInput : null,
      );
      if (result.status === "error") {
        setErrorMessage(result.error);
        return;
      }
      setCredentialsInput("");
      setClientSecretInput("");
      setEditingCredentials(false);
      await queryClient.invalidateQueries({
        queryKey: LOCAL_GOOGLE_STATUS_QUERY_KEY,
      });
    } catch (error) {
      setErrorMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setIsSaving(false);
    }
  }, [clientSecretInput, credentialsInput, queryClient]);

  const handleConnect = useCallback(async () => {
    setErrorMessage(null);
    setIsConnecting(true);
    try {
      allowReconnectedCalendarConnections("google-calendar");
      const result = await calendarCommands.connectLocalGoogleCalendar();
      if (result.status === "error") {
        setErrorMessage(result.error);
        return;
      }
      await refreshAll();
    } catch (error) {
      setErrorMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setIsConnecting(false);
    }
  }, [refreshAll]);

  const handleDisconnect = useCallback(
    async (connectionId: string) => {
      setErrorMessage(null);
      try {
        const result =
          await calendarCommands.disconnectLocalGoogleCalendar(connectionId);
        if (result.status === "error") {
          setErrorMessage(result.error);
          return;
        }
        await removeDisconnectedCalendarConnection(
          "google-calendar",
          connectionId,
        );
        await refreshAll();
      } catch (error) {
        setErrorMessage(error instanceof Error ? error.message : String(error));
      }
    },
    [refreshAll],
  );

  const { groups, handleRefresh, handleToggle, isLoading } =
    useOAuthCalendarSelection(config, connections);

  const groupsWithMenus = useMemo(
    () =>
      groups.map((group) => {
        const connection = connections.find(
          (item) => item.connection_id === group.id,
        );
        if (!connection) return group;

        return {
          ...group,
          menuItems: [
            {
              id: `reconnect-${connection.connection_id}`,
              text: "Reconnect",
              action: () => {
                void handleConnect();
              },
            },
            {
              id: `disconnect-${connection.connection_id}`,
              text: "Disconnect",
              action: () => {
                void handleDisconnect(connection.connection_id);
              },
            },
          ],
        };
      }),
    [connections, groups, handleConnect, handleDisconnect],
  );

  if (statusQuery.isPending) {
    return (
      <div className="text-muted-foreground flex items-center gap-1.5 pt-1 pb-2 text-xs">
        <CircleNotch className="size-3 animate-spin" aria-hidden="true" />
        <span>Loading…</span>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2 pt-1 pb-2">
      {showSetupForm ? (
        <div className="border-border bg-card/50 flex flex-col gap-2 rounded-lg border p-2.5 text-xs">
          <p className="text-muted-foreground leading-relaxed">
            Direct local OAuth (no cloud server). Paste your Google Cloud{" "}
            <span className="text-foreground font-medium">Desktop app</span>{" "}
            OAuth JSON or Client ID below.
          </p>
          <textarea
            value={credentialsInput}
            onChange={(event) => setCredentialsInput(event.target.value)}
            placeholder='Paste client_secret_*.json or Client ID (…apps.googleusercontent.com)'
            rows={3}
            className="border-border bg-background text-foreground placeholder:text-muted-foreground w-full resize-none rounded-md border px-2 py-1.5 font-mono text-[11px] focus:outline-none"
          />
          {!credentialsInput.trim().startsWith("{") && (
            <input
              type="password"
              value={clientSecretInput}
              onChange={(event) => setClientSecretInput(event.target.value)}
              placeholder="Client Secret (optional if pasted in JSON)"
              className="border-border bg-background text-foreground placeholder:text-muted-foreground w-full rounded-md border px-2 py-1.5 font-mono text-[11px] focus:outline-none"
            />
          )}
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleSaveCredentials()}
              disabled={isSaving || !credentialsInput.trim()}
              className="bg-foreground text-background inline-flex cursor-pointer items-center gap-1 rounded-md px-2.5 py-1 text-xs font-medium disabled:opacity-50"
            >
              {isSaving && (
                <CircleNotch className="size-3 animate-spin" aria-hidden="true" />
              )}
              Save credentials
            </button>
            {configured && (
              <button
                type="button"
                onClick={() => {
                  setEditingCredentials(false);
                  setErrorMessage(null);
                }}
                className="text-muted-foreground hover:text-foreground cursor-pointer text-xs underline"
              >
                Cancel
              </button>
            )}
          </div>
        </div>
      ) : (
        <>
          {connections.length > 0 ? (
            <OAuthCalendarSelection
              groups={groupsWithMenus}
              onToggle={handleToggle}
              onRefresh={handleRefresh}
              isLoading={isLoading}
            />
          ) : null}

          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <button
              type="button"
              onClick={() => void handleConnect()}
              disabled={isConnecting}
              className="text-muted-foreground hover:text-foreground inline-flex cursor-pointer items-center gap-1 text-xs underline transition-colors disabled:opacity-50"
            >
              {isConnecting && (
                <CircleNotch className="size-3 animate-spin" aria-hidden="true" />
              )}
              {isConnecting
                ? "Waiting for Google sign-in…"
                : connections.length > 0
                  ? "Add Google account"
                  : "Connect Google Calendar"}
            </button>
            <button
              type="button"
              onClick={() => {
                setEditingCredentials(true);
                setErrorMessage(null);
              }}
              className="text-muted-foreground hover:text-foreground cursor-pointer text-[11px] underline transition-colors"
            >
              OAuth credentials
            </button>
          </div>
        </>
      )}

      {errorMessage && (
        <p role="alert" className="text-xs text-red-600">
          {errorMessage}
        </p>
      )}
    </div>
  );
}
