// LOCAL-GRANOLA: which enhance pipeline a build uses. Kept free of imports
// from the task-config index so it can be evaluated while that module loads.

import { LOCAL_ONLY } from "~/shared/local-only";

export type EnhanceRoute = "local" | "stream";

/** LOCAL_ONLY builds enhance via the Rust `enhance_local` command. */
export function resolveEnhanceRoute(localOnly: boolean): EnhanceRoute {
  return localOnly ? "local" : "stream";
}

export function shouldEnhanceLocally(): boolean {
  return resolveEnhanceRoute(LOCAL_ONLY) === "local";
}
