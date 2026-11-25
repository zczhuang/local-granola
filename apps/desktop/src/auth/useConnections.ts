import { useQuery } from "@tanstack/react-query";

import { listConnections, type ConnectionItem } from "@anlg/api-client";
import { createClient } from "@anlg/api-client/client";
import { commands as calendarCommands } from "@anlg/plugin-calendar";

import { useAuth } from "./auth-context";

import { env } from "~/env";
import { LOCAL_ONLY } from "~/shared/local-only";

export function useConnections(
  enabled = true,
  options?: { refetchInterval?: number | false },
) {
  const auth = useAuth();
  const userId = auth?.session?.user.id;

  // eslint-disable-next-line @tanstack/query/exhaustive-deps -- Auth supplies request headers; the user ID is the connection-list identity.
  return useQuery<ConnectionItem[]>({
    queryKey: ["integration-status", LOCAL_ONLY ? "local" : userId],
    queryFn: async () => {
      if (LOCAL_ONLY) {
        const result = await calendarCommands.getLocalGoogleCalendarStatus();
        if (result.status === "error") {
          throw new Error(result.error);
        }
        return result.data.accounts.map((account) => ({
          integration_id: "google-calendar",
          connection_id: account.connection_id,
          account_identity: account.email,
          status: "active",
        }));
      }

      const headers = auth?.getHeaders();
      if (!headers) {
        return [];
      }
      const client = createClient({ baseUrl: env.VITE_API_URL, headers });
      const { data, error } = await listConnections({ client });
      if (error) {
        throw new Error("Failed to load integrations");
      }
      return data?.connections ?? [];
    },
    enabled: LOCAL_ONLY ? true : enabled && !!userId,
    refetchInterval: options?.refetchInterval,
  });
}
