import { isLocalUrl, LOCAL_STT_PROVIDERS } from "~/shared/local-only";

export type SttConnectionCandidate = {
  provider: string;
  baseUrl: string;
};

const ON_DEVICE_LOCAL_URLS: Readonly<Record<string, string>> = {
  soniqo: "soniqo://local",
  apple_speech: "apple-speech://local",
};

/**
 * Local-only egress guard for STT connections. Returns true when the connection
 * may be used: a local provider, not the hosted cloud model, and either its
 * in-process on-device sentinel URL (`soniqo://local`, `apple-speech://local`)
 * or a loopback base URL. Always true when `localOnly` is false.
 */
export function isSttConnectionAllowed(
  candidate: SttConnectionCandidate,
  options: { localOnly: boolean; isCloudModel: boolean },
): boolean {
  if (!options.localOnly) return true;
  if (options.isCloudModel) return false;
  if (!LOCAL_STT_PROVIDERS.includes(candidate.provider)) return false;
  const normalizedBaseUrl = candidate.baseUrl.trim().replace(/\/+$/, "");
  const onDeviceUrl = ON_DEVICE_LOCAL_URLS[candidate.provider];
  if (onDeviceUrl && normalizedBaseUrl === onDeviceUrl) {
    return true;
  }
  return isLocalUrl(candidate.baseUrl);
}
