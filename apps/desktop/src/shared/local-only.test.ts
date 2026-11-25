import { afterEach, describe, expect, it, vi } from "vitest";

import {
  assertLocalUrl,
  filterLocalProviders,
  isLocalLlmProvider,
  isLocalUrl,
  LOCAL_LLM_PROVIDERS,
  LOCAL_STT_PROVIDERS,
  LocalOnlyError,
} from "./local-only";

describe("isLocalUrl", () => {
  it.each([
    "http://localhost:11434/v1",
    "http://LOCALHOST/",
    "http://127.0.0.1:11434/v1",
    "http://127.8.9.10:1234",
    "http://[::1]:8080/x",
    "ws://localhost:9000/stream",
  ])("accepts loopback %s", (url) => {
    expect(isLocalUrl(url)).toBe(true);
  });

  it.each([
    "https://api.openai.com/v1",
    "http://localhost.evil.com/",
    "http://127.0.0.1.evil.com/",
    "http://192.168.1.5:11434",
    "http://0.0.0.0:11434",
    "https://user@evil.com@localhost.evil.com",
    "file:///etc/passwd",
    "not a url",
    "",
  ])("rejects %s", (url) => {
    expect(isLocalUrl(url)).toBe(false);
  });
});

describe("assertLocalUrl", () => {
  it("throws for remote hosts in local-only mode without leaking the path", () => {
    expect(() =>
      assertLocalUrl("https://api.openai.com/v1/chat?key=secret", true),
    ).toThrow(LocalOnlyError);
    try {
      assertLocalUrl("https://api.openai.com/v1/chat?key=secret", true);
    } catch (error) {
      expect(String(error)).not.toContain("secret");
    }
  });

  it("allows loopback and, when disabled, everything", () => {
    expect(() =>
      assertLocalUrl("http://127.0.0.1:11434/v1", true),
    ).not.toThrow();
    expect(() => assertLocalUrl("https://api.openai.com", false)).not.toThrow();
  });
});

describe("provider filtering", () => {
  const providers = [
    { id: "anarlog" },
    { id: "openai" },
    { id: "ollama" },
    { id: "lmstudio" },
    { id: "apple_foundation" },
  ];

  it("keeps only local providers when local-only", () => {
    expect(
      filterLocalProviders(providers, LOCAL_LLM_PROVIDERS, true).map(
        (p) => p.id,
      ),
    ).toEqual(["ollama", "lmstudio", "apple_foundation"]);
  });

  it("keeps everything otherwise", () => {
    expect(filterLocalProviders(providers, LOCAL_LLM_PROVIDERS, false)).toEqual(
      providers,
    );
  });

  it("does not treat a custom STT endpoint as local", () => {
    expect(LOCAL_STT_PROVIDERS).not.toContain("custom");
    expect(
      filterLocalProviders(
        [{ id: "soniqo" }, { id: "custom" }],
        LOCAL_STT_PROVIDERS,
        true,
      ).map((p) => p.id),
    ).toEqual(["soniqo"]);
  });

  it("never lists a hosted provider as local", () => {
    for (const id of [
      "anarlog",
      "openai",
      "anthropic",
      "deepgram",
      "openrouter",
    ]) {
      expect(isLocalLlmProvider(id)).toBe(false);
      expect(LOCAL_STT_PROVIDERS).not.toContain(id);
    }
  });
});

describe("with local-only enabled for real", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("defaults on unless VITE_LOCAL_ONLY is exactly false", async () => {
    vi.stubEnv("VITE_LOCAL_ONLY", "");
    vi.resetModules();
    expect((await import("./local-only")).LOCAL_ONLY).toBe(true);
    vi.stubEnv("VITE_LOCAL_ONLY", "false");
    vi.resetModules();
    expect((await import("./local-only")).LOCAL_ONLY).toBe(false);
  });

  it("removes hosted providers from the real LLM and STT lists", async () => {
    vi.stubEnv("VITE_LOCAL_ONLY", "true");
    vi.resetModules();
    vi.doMock("@lobehub/icons", () =>
      Object.fromEntries(
        [
          "AlibabaCloud",
          "Anthropic",
          "Apple",
          "AssemblyAI",
          "Aws",
          "Azure",
          "AzureAI",
          "Bedrock",
          "Cerebras",
          "Claude",
          "Cohere",
          "DeepSeek",
          "ElevenLabs",
          "Fireworks",
          "Gemini",
          "GithubCopilot",
          "GoogleCloud",
          "Grok",
          "Groq",
          "LmStudio",
          "Mistral",
          "Moonshot",
          "Nvidia",
          "Ollama",
          "OpenAI",
          "OpenRouter",
          "SiliconCloud",
          "Together",
          "VertexAI",
          "WorkersAI",
          "XAI",
          "ZAI",
        ].map((name) => [name, () => null]),
      ),
    );
    vi.doMock("~/settings/ai/shared", () => ({
      AnarlogProviderIcon: () => null,
      ProviderBrandImage: () => null,
      ProviderLobeIcon: () => null,
    }));
    vi.doMock("~/stt/useLocalSttModel", () => ({ localSttQueries: {} }));
    const llm = (await import("~/settings/ai/llm/shared")).PROVIDERS.map(
      (p) => p.id,
    );
    const stt = (await import("~/settings/ai/stt/shared")).PROVIDERS.map(
      (p) => p.id,
    );
    expect(llm).toEqual(expect.arrayContaining(["ollama", "lmstudio"]));
    for (const hosted of ["anarlog", "openai", "anthropic", "openrouter"]) {
      expect(llm).not.toContain(hosted);
    }
    expect(stt).toEqual(expect.arrayContaining(["soniqo", "apple_speech"]));
    expect(stt).not.toContain("custom");
    for (const hosted of ["anarlog", "deepgram", "openai", "assemblyai"]) {
      expect(stt).not.toContain(hosted);
    }
  }, 30_000);

  it("blocks remote provider fetches before any network call", async () => {
    vi.stubEnv("VITE_LOCAL_ONLY", "true");
    vi.resetModules();
    const tauriFetch = vi.fn().mockResolvedValue(new Response("ok"));
    vi.doMock("@tauri-apps/plugin-http", () => ({ fetch: tauriFetch }));
    const { providerFetch } = await import("~/ai/provider-fetch");
    await expect(
      providerFetch("https://api.openai.com/v1/chat/completions"),
    ).rejects.toBeInstanceOf(Error);
    expect(tauriFetch).not.toHaveBeenCalled();
    await providerFetch("http://127.0.0.1:11434/v1/models");
    expect(tauriFetch).toHaveBeenCalledTimes(1);
  });
});
