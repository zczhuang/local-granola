import { platform } from "@tauri-apps/plugin-os";

import type { SectionStatus } from "./shared";

import { LOCAL_ONLY } from "~/shared/local-only";

export type OnboardingStep =
  | "permissions"
  | "login"
  | "calendar"
  | "imports"
  | "final";

const STEPS_MACOS: OnboardingStep[] = [
  "permissions",
  "login",
  "calendar",
  "imports",
  "final",
];
const STEPS_OTHER: OnboardingStep[] = ["login", "calendar", "imports", "final"];

/** The local-only build has no account, so onboarding skips the login step. */
export function withoutLoginWhenLocalOnly(
  steps: OnboardingStep[],
  localOnly: boolean = LOCAL_ONLY,
): OnboardingStep[] {
  return localOnly ? steps.filter((step) => step !== "login") : steps;
}

function getOnboardingSteps(): OnboardingStep[] {
  return withoutLoginWhenLocalOnly(
    platform() === "macos" ? STEPS_MACOS : STEPS_OTHER,
  );
}

export function getInitialStep(): OnboardingStep {
  return getOnboardingSteps()[0];
}

export function getNextStep(
  currentStep: OnboardingStep,
): OnboardingStep | null {
  const steps = getOnboardingSteps();
  const idx = steps.indexOf(currentStep);
  return idx < steps.length - 1 ? steps[idx + 1] : null;
}

export function getPrevStep(
  currentStep: OnboardingStep,
): OnboardingStep | null {
  const steps = getOnboardingSteps();
  const idx = steps.indexOf(currentStep);
  return idx > 0 ? steps[idx - 1] : null;
}

export function getStepStatus(
  step: OnboardingStep,
  currentStep: OnboardingStep,
): SectionStatus | null {
  const steps = getOnboardingSteps();
  const stepIdx = steps.indexOf(step);
  if (stepIdx === -1) return null;
  const currentIdx = steps.indexOf(currentStep);
  if (stepIdx < currentIdx) return "completed";
  if (stepIdx === currentIdx) return "active";
  return "upcoming";
}
