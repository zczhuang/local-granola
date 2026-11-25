import { expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-os", () => ({ platform: () => "macos" }));

import { withoutLoginWhenLocalOnly, type OnboardingStep } from "./config";

const steps: OnboardingStep[] = [
  "permissions",
  "login",
  "calendar",
  "imports",
  "final",
];

it("drops the login step only in local-only mode", () => {
  expect(withoutLoginWhenLocalOnly(steps, true)).toEqual([
    "permissions",
    "calendar",
    "imports",
    "final",
  ]);
  expect(withoutLoginWhenLocalOnly(steps, false)).toEqual(steps);
});
