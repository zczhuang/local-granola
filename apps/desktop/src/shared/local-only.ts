// Single switch for the personal local-only build. On by default; set
// VITE_LOCAL_ONLY=false at build time to restore the hosted behavior.
// Rust counterpart: crates/local-only (ANARLOG_LOCAL_ONLY).

export const LOCAL_ONLY: boolean = import.meta.env.VITE_LOCAL_ONLY !== "false";

/** LLM providers that run on this machine (or at a loopback address). */
export const LOCAL_LLM_PROVIDERS: readonly string[] = [
  "ollama",
  "lmstudio",
  "unsloth",
  "apple_foundation",
  // User-supplied OpenAI-compatible endpoint; every request is still checked
  // by assertLocalUrl, so only loopback URLs can ever be reached.
  "custom",
];

/**
 * STT providers that run on this machine. "custom" is deliberately absent: its
 * base URL can be a remote websocket, which the TS fetch guard does not cover.
 */
export const LOCAL_STT_PROVIDERS: readonly string[] = [
  "soniqo",
  "apple_speech",
  "local_file",
];

export const DEFAULT_LOCAL_LLM_PROVIDER = "ollama";
export const DEFAULT_LOCAL_LLM_MODEL = "gemma4:26b-mlx";

const LOOPBACK_IPV4 = /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/;

/** True only for http(s)/ws(s) URLs whose host is loopback. */
export function isLocalUrl(input: string | URL): boolean {
  let url: URL;
  try {
    url = typeof input === "string" ? new URL(input) : input;
  } catch {
    return false;
  }
  if (!["http:", "https:", "ws:", "wss:"].includes(url.protocol)) return false;
  const host = url.hostname.toLowerCase();
  return host === "localhost" || host === "[::1]" || LOOPBACK_IPV4.test(host);
}

export class LocalOnlyError extends Error {
  constructor(url: string) {
    super(`Blocked non-local network request in local-only mode: ${url}`);
    this.name = "LocalOnlyError";
  }
}

/** Throws unless the URL is loopback. No-op when `localOnly` is false. */
export function assertLocalUrl(
  input: string | URL,
  localOnly: boolean = LOCAL_ONLY,
): void {
  if (!localOnly || isLocalUrl(input)) return;
  let shown = String(input);
  try {
    const url = new URL(shown);
    shown = `${url.protocol}//${url.host}`;
  } catch {
    // keep the raw string; it carries no query/credentials we can parse
  }
  throw new LocalOnlyError(shown);
}

/** Keeps only `allowed` provider ids when `localOnly`; otherwise returns all. */
export function filterLocalProviders<T extends { id: string }>(
  providers: readonly T[],
  allowed: readonly string[],
  localOnly: boolean = LOCAL_ONLY,
): T[] {
  return localOnly
    ? providers.filter((provider) => allowed.includes(provider.id))
    : [...providers];
}

export function isLocalLlmProvider(providerId: string): boolean {
  return LOCAL_LLM_PROVIDERS.includes(providerId);
}
