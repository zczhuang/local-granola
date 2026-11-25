import { Trans, useLingui } from "@lingui/react/macro";
import { type ReactNode, useState } from "react";

import { Input } from "@anlg/ui/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@anlg/ui/components/ui/select";

import {
  audioRetentionDays,
  controlsToRetention,
  MAX_AUDIO_RETENTION_DAYS,
  normalizeAudioRetention,
  retentionToControls,
} from "~/services/audio-retention-policy";
import {
  SETTING_CONTROL_CLASS,
  SettingRow,
  SettingSwitchRow,
} from "~/settings/setting-row";

export function AudioSettingsView({
  audioRetention,
  microphoneDevice,
  rememberSpeakers,
}: {
  audioRetention: {
    value: string;
    onChange: (value: string) => void;
  };
  microphoneDevice: {
    value: string;
    devices: string[];
    onChange: (value: string) => void;
  };
  rememberSpeakers: {
    value: boolean;
    onChange: (value: boolean) => void;
  };
}) {
  return (
    <div className="flex flex-col gap-4">
      <AudioDeviceRow
        title={<Trans>Microphone</Trans>}
        description={
          <Trans>Choose the microphone that captures your voice.</Trans>
        }
        value={microphoneDevice.value}
        devices={microphoneDevice.devices}
        onChange={microphoneDevice.onChange}
      />
      <AudioRetentionRow
        value={audioRetention.value}
        onChange={audioRetention.onChange}
      />
      <SettingSwitchRow
        title={<Trans>Remember speakers</Trans>}
        description={
          <Trans>
            Build voiceprints from meeting audio so speakers you name in a
            transcript are recognized in later meetings. Voiceprints never leave
            this device, and unnamed ones are deleted after 45 days.
          </Trans>
        }
        checked={rememberSpeakers.value}
        onChange={rememberSpeakers.onChange}
      />
    </div>
  );
}

const SYSTEM_DEFAULT_DEVICE = "__system_default_device__";

export function AudioDeviceRow({
  title,
  description,
  value,
  devices,
  onChange,
}: {
  title: ReactNode;
  description: ReactNode;
  value: string;
  devices: string[];
  onChange: (value: string) => void;
}) {
  const availableDevices = [...new Set(devices)].sort((a, b) =>
    a.localeCompare(b),
  );
  const selectedDeviceUnavailable =
    Boolean(value) && !availableDevices.includes(value);
  if (selectedDeviceUnavailable) {
    availableDevices.unshift(value);
  }

  return (
    <SettingRow title={title} description={description}>
      {(labelProps) => (
        <Select
          value={value || SYSTEM_DEFAULT_DEVICE}
          onValueChange={(device) =>
            onChange(device === SYSTEM_DEFAULT_DEVICE ? "" : device)
          }
        >
          <SelectTrigger {...labelProps} className={SETTING_CONTROL_CLASS}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className="max-h-64">
            <SelectItem value={SYSTEM_DEFAULT_DEVICE}>
              <Trans>Current default</Trans>
            </SelectItem>
            {availableDevices.map((device) => (
              <SelectItem key={device} value={device}>
                {device}
                {selectedDeviceUnavailable && device === value ? (
                  <>
                    {" "}
                    <Trans>(Unavailable — using current default)</Trans>
                  </>
                ) : null}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    </SettingRow>
  );
}

function AudioRetentionRow({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const { t } = useLingui();
  const controls = retentionToControls(normalizeAudioRetention(value));
  const [draftDays, setDraftDays] = useState<string | null>(null);
  const shownDays = draftDays ?? (controls.days === null ? "" : controls.days);

  return (
    <>
      <SettingSwitchRow
        title={<Trans>Delete audio after transcription</Trans>}
        description={
          <Trans>
            Remove the recording from this device as soon as the transcript is
            done. Transcripts and notes are kept.
          </Trans>
        }
        checked={controls.deleteAfter}
        onChange={(deleteAfter) => {
          setDraftDays(null);
          onChange(controlsToRetention({ deleteAfter, days: null }));
        }}
      />
      {controls.deleteAfter && (
        <SettingRow
          title={<Trans>Keep audio for (days)</Trans>}
          description={
            <Trans>
              Optional. Leave empty to delete right after transcription.
            </Trans>
          }
        >
          {(labelProps) => (
            <Input
              {...labelProps}
              type="number"
              inputMode="numeric"
              min={1}
              max={MAX_AUDIO_RETENTION_DAYS}
              step={1}
              placeholder={t`Delete immediately`}
              className={SETTING_CONTROL_CLASS}
              value={shownDays}
              onChange={(event) => {
                const text = event.target.value;
                setDraftDays(text);
                const days = text.trim() === "" ? null : Number(text);
                if (days === null || audioRetentionDays(days) !== null) {
                  onChange(controlsToRetention({ deleteAfter: true, days }));
                }
              }}
              onBlur={() => setDraftDays(null)}
            />
          )}
        </SettingRow>
      )}
    </>
  );
}
