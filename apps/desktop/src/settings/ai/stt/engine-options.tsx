import { Trans, useLingui } from "@lingui/react/macro";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@anlg/ui/components/ui/select";

import { useSetSettingValues } from "~/settings/queries";
import { SETTING_CONTROL_CLASS, SettingRow } from "~/settings/setting-row";
import { useConfigValue } from "~/shared/config";
import { normalizeSttEngineOverride } from "~/stt/engine-routing";
import { normalizeWhisperRepass } from "~/stt/whisper-repass";

// LOCAL-GRANOLA: controls for the post-meeting Whisper re-pass and the live engine override.
export function SttEngineOptions() {
  const { t } = useLingui();
  const setSettingValues = useSetSettingValues();
  const repass = normalizeWhisperRepass(useConfigValue("stt_whisper_repass"));
  const override = normalizeSttEngineOverride(
    useConfigValue("stt_engine_override"),
  );

  const repassLabels = {
    off: t`Off`,
    auto: t`Automatic`,
    always: t`Always`,
  } as const;
  const overrideLabels = {
    auto: t`Automatic`,
    parakeet: t`Parakeet only`,
    whisper: t`Whisper`,
  } as const;

  return (
    <div className="flex flex-col gap-4">
      <SettingRow
        title={<Trans>Whisper pass after meetings</Trans>}
        description={
          <Trans>
            Re-transcribe the recording with Whisper large-v3-turbo once the
            meeting ends. Automatic does this when live text came from a fast
            on-device engine.
          </Trans>
        }
      >
        {(labelProps) => (
          <Select
            value={repass}
            onValueChange={(value) =>
              setSettingValues({ stt_whisper_repass: value })
            }
          >
            <SelectTrigger {...labelProps} className={SETTING_CONTROL_CLASS}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(Object.keys(repassLabels) as (keyof typeof repassLabels)[]).map(
                (option) => (
                  <SelectItem key={option} value={option}>
                    {repassLabels[option]}
                  </SelectItem>
                ),
              )}
            </SelectContent>
          </Select>
        )}
      </SettingRow>
      <SettingRow
        title={<Trans>Language routing</Trans>}
        description={
          <Trans>
            Automatic detects Mandarin or mixed-language meetings and uses
            Whisper for the final transcript. Pick an engine to skip detection.
          </Trans>
        }
      >
        {(labelProps) => (
          <Select
            value={override}
            onValueChange={(value) =>
              setSettingValues({ stt_engine_override: value })
            }
          >
            <SelectTrigger {...labelProps} className={SETTING_CONTROL_CLASS}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(
                Object.keys(overrideLabels) as (keyof typeof overrideLabels)[]
              ).map((option) => (
                <SelectItem key={option} value={option}>
                  {overrideLabels[option]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </SettingRow>
    </div>
  );
}
