import { describe, expect, it } from "vitest";

import { isSttConnectionAllowed } from "./local-only-guard";

const local = { localOnly: true, isCloudModel: false };

describe("isSttConnectionAllowed", () => {
  it("allows a local provider on loopback or its in-process on-device URL", () => {
    expect(
      isSttConnectionAllowed(
        { provider: "soniqo", baseUrl: "http://127.0.0.1:5000" },
        local,
      ),
    ).toBe(true);
    expect(
      isSttConnectionAllowed(
        { provider: "soniqo", baseUrl: "soniqo://local" },
        local,
      ),
    ).toBe(true);
    expect(
      isSttConnectionAllowed(
        { provider: "apple_speech", baseUrl: "apple-speech://local" },
        local,
      ),
    ).toBe(true);
    expect(
      isSttConnectionAllowed(
        { provider: "soniqo", baseUrl: "apple-speech://local" },
        local,
      ),
    ).toBe(false);
  });

  it("blocks the hosted cloud model", () => {
    expect(
      isSttConnectionAllowed(
        { provider: "soniqo", baseUrl: "http://127.0.0.1:5000" },
        { localOnly: true, isCloudModel: true },
      ),
    ).toBe(false);
  });

  it("blocks providers outside the local list, even on loopback", () => {
    expect(
      isSttConnectionAllowed(
        { provider: "nvidia", baseUrl: "http://localhost:9000" },
        local,
      ),
    ).toBe(false);
  });

  it("blocks non-loopback base URLs", () => {
    expect(
      isSttConnectionAllowed(
        { provider: "soniqo", baseUrl: "https://api.deepgram.com/v1" },
        local,
      ),
    ).toBe(false);
    expect(
      isSttConnectionAllowed({ provider: "soniqo", baseUrl: "" }, local),
    ).toBe(false);
  });

  it("allows everything when local-only is off", () => {
    expect(
      isSttConnectionAllowed(
        { provider: "deepgram", baseUrl: "https://api.deepgram.com/v1" },
        { localOnly: false, isCloudModel: true },
      ),
    ).toBe(true);
  });
});
