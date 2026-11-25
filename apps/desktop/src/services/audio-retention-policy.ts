const AUDIO_RETENTION_DURATION_MS = {
  none: 0,
  oneDay: 24 * 60 * 60 * 1000,
  threeDays: 3 * 24 * 60 * 60 * 1000,
  oneWeek: 7 * 24 * 60 * 60 * 1000,
  oneMonth: 30 * 24 * 60 * 60 * 1000,
} as const;

type ExpiringAudioRetentionPolicy = keyof typeof AUDIO_RETENTION_DURATION_MS;
export type AudioRetentionDays = `days:${number}`;
export type AudioRetentionPolicy =
  | ExpiringAudioRetentionPolicy
  | AudioRetentionDays
  | "forever";

const AUDIO_RETENTION_VALUES = new Set([
  ...Object.keys(AUDIO_RETENTION_DURATION_MS),
  "forever",
]);

/** Mirrors `MAX_DAYS` in crates/audio-retention. */
export const MAX_AUDIO_RETENTION_DAYS = 3650;

const DAYS_PATTERN = /^days:(\d+)$/;

/** `days:N` (1 <= N <= 3650) to N, anything else to null. */
export function parseAudioRetentionDays(value: unknown): number | null {
  if (typeof value !== "string") return null;
  const match = DAYS_PATTERN.exec(value);
  if (!match) return null;
  const days = Number(match[1]);
  return Number.isSafeInteger(days) &&
    days >= 1 &&
    days <= MAX_AUDIO_RETENTION_DAYS
    ? days
    : null;
}

export function audioRetentionDays(days: number): AudioRetentionDays | null {
  const value = `days:${Math.trunc(days)}` as const;
  return parseAudioRetentionDays(value) === null ? null : value;
}

export function normalizeAudioRetention<
  T extends AudioRetentionPolicy | undefined = "forever",
>(value: unknown, fallback?: T): AudioRetentionPolicy | T {
  if (typeof value === "string" && AUDIO_RETENTION_VALUES.has(value)) {
    return value as AudioRetentionPolicy;
  }

  if (parseAudioRetentionDays(value) !== null) {
    return value as AudioRetentionDays;
  }

  if (value === false) {
    return "none";
  }

  if (value === true) {
    return "forever";
  }

  return arguments.length >= 2 ? (fallback as T) : ("forever" as T);
}

const LEGACY_RETENTION_DAYS = {
  oneDay: 1,
  threeDays: 3,
  oneWeek: 7,
  oneMonth: 30,
} as const;

export type AudioRetentionControls = {
  /** "Delete audio after transcription" toggle. */
  deleteAfter: boolean;
  /** Optional "keep for N days" grace period; null deletes right away. */
  days: number | null;
};

export function retentionToControls(
  policy: AudioRetentionPolicy,
): AudioRetentionControls {
  if (policy === "forever") return { deleteAfter: false, days: null };
  if (policy === "none") return { deleteAfter: true, days: null };
  if (policy in LEGACY_RETENTION_DAYS) {
    return {
      deleteAfter: true,
      days: LEGACY_RETENTION_DAYS[policy as keyof typeof LEGACY_RETENTION_DAYS],
    };
  }
  return { deleteAfter: true, days: parseAudioRetentionDays(policy) };
}

export function controlsToRetention({
  deleteAfter,
  days,
}: AudioRetentionControls): AudioRetentionPolicy {
  if (!deleteAfter) return "forever";
  if (days === null || !Number.isFinite(days)) return "none";
  return audioRetentionDays(days) ?? "none";
}

/**
 * Whether a capture records audio to disk. The local-only build always records
 * (the Whisper re-pass and the audio catalog need the file; "none" then
 * deletes it once processing is done). Otherwise only when the retention
 * policy keeps audio or the STT model needs a retained batch pass.
 */
export function shouldRetainAudio({
  localOnly,
  audioRetention,
  requiresBatchAudio,
}: {
  localOnly: boolean;
  audioRetention: AudioRetentionPolicy;
  requiresBatchAudio: boolean;
}): boolean {
  return localOnly || audioRetention !== "none" || requiresBatchAudio;
}
