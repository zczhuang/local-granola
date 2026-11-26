import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  platform: vi.fn(() => "macos"),
  setAppIcon: vi.fn(),
  applyAppIconPreference: vi.fn(),
  appIcon: "default",
  theme: "system",
  appIdentifier: "com.hyprnote.stable" as string | undefined,
  billing: {
    isPro: true,
    isUpgradingToPro: false,
    upgradeToPro: vi.fn(),
  },
  toastWarning: vi.fn(),
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: mocks.appIdentifier }),
}));

vi.mock("@tauri-apps/api/app", () => ({
  getIdentifier: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-os", () => ({
  platform: mocks.platform,
}));

vi.mock("~/settings/queries", () => ({
  useSetSettingValue: () => mocks.setAppIcon,
}));

vi.mock("~/auth/billing-context", () => ({
  useBillingAccess: () => mocks.billing,
}));

vi.mock("@anlg/ui/components/ui/toast", () => ({
  toast: { warning: mocks.toastWarning },
}));

vi.mock("~/shared/config", () => ({
  useConfigValue: (key: string) =>
    key === "theme" ? mocks.theme : mocks.appIcon,
}));

vi.mock("~/shared/theme/provider", () => ({
  applyAppIconPreference: mocks.applyAppIconPreference,
}));

import { AppIconSelector } from "./app-icon";

describe("AppIconSelector", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
    mocks.platform.mockReturnValue("macos");
    mocks.appIcon = "default";
    mocks.theme = "system";
    mocks.appIdentifier = "com.hyprnote.stable";
    mocks.billing.isPro = true;
    mocks.billing.isUpgradingToPro = false;
    mocks.toastWarning.mockClear();
  });

  it.each([["system"], ["dark"]])(
    "applies and stores the selected Oatmeal icon for the %s theme",
    (theme) => {
      mocks.theme = theme;
      render(<AppIconSelector />);

      expect(
        screen
          .getByRole("radio", { name: "Classic Oat" })
          .getAttribute("aria-checked"),
      ).toBe("true");

      fireEvent.click(screen.getByRole("radio", { name: "Obsidian Slate" }));

      expect(mocks.applyAppIconPreference).toHaveBeenCalledWith("stone", theme);
      expect(mocks.setAppIcon).toHaveBeenCalledWith("stone");
    },
  );

  it("allows selecting any Oatmeal icon without a Pro gate and hides legacy icons", () => {
    mocks.billing.isPro = false;

    render(<AppIconSelector />);

    expect(screen.queryByRole("radio", { name: "Anagram" })).toBeNull();
    expect(screen.queryByRole("radio", { name: "Blueprint" })).toBeNull();
    expect(screen.queryByRole("radio", { name: "Sketch" })).toBeNull();

    const leatherOption = screen.getByRole("radio", {
      name: "Espresso Leather",
    });
    fireEvent.click(leatherOption);

    expect(mocks.toastWarning).not.toHaveBeenCalled();
    expect(mocks.applyAppIconPreference).toHaveBeenCalledWith(
      "journal",
      "system",
    );
    expect(mocks.setAppIcon).toHaveBeenCalledWith("journal");
  });

  it("selects Classic Oat for an equivalent channel-specific preference", () => {
    mocks.appIcon = "stable";

    render(<AppIconSelector />);

    expect(
      screen
        .getByRole("radio", { name: "Classic Oat" })
        .getAttribute("aria-checked"),
    ).toBe("true");
    expect(screen.queryByRole("radio", { name: "Production" })).toBeNull();
  });
});
