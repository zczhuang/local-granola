// LOCAL-GRANOLA: enhanced notes in the local-only build. Instead of streaming
// a summary from an LLM provider, the Rust `enhance_local` command keeps the
// user's typed notes as the skeleton, asks a local Ollama model for
// transcript-linked bullets, and saves the document itself.

import type { LanguageModel } from "ai";

import { commands as localApiCommands } from "@anlg/plugin-local-api";
import { commands as templateCommands } from "@anlg/plugin-template";

import { createTaskId, type LocalEnhanceArgs, type TaskConfig } from ".";

import { runNoteEnhancedAutomations } from "~/automations/engine";
import { syncCloudApiSnapshotBestEffort } from "~/cloud-api/client";
import { loadSessionContentSnapshot } from "~/session/content-queries";
import { requestAppAttention } from "~/shared/app-attention";
import { playCompletionSound } from "~/shared/completion-sound";
import { hasLiveSessionTitleDraft } from "~/store/zustand/live-title";
import { isGenericSessionTitle } from "~/stt/meeting-metadata-sync";

const HEARTBEAT_MS = 10_000;

/**
 * Runs `enhance_local` and yields the saved document as markdown (used for
 * title generation). A heartbeat chunk keeps the task's stream-stall timeout
 * from firing while a long meeting is processed in chunks.
 */
export async function* executeLocalEnhance(params: {
  args: LocalEnhanceArgs;
  onProgress: (step: any) => void;
  signal: AbortSignal;
}): AsyncGenerator<any> {
  const { args, onProgress, signal } = params;
  onProgress({ type: "generating" });

  const pending = templateCommands.enhanceLocal({
    session_id: args.sessionId,
    note_id: args.enhancedNoteId,
    regenerate: args.regenerate,
    owner_user_id: null,
    model: null,
    base_url: null,
    num_ctx: null,
    chinese_script: null,
    pending_auto_enhance: args.pendingAutoEnhance
      ? {
          generation: args.pendingAutoEnhance.generation,
          expected_body: args.pendingAutoEnhance.expectedBody,
          expected_body_format: args.pendingAutoEnhance.expectedContentFormat,
        }
      : null,
  });
  const done = pending.then(
    (result) => ({ result }),
    (error: unknown) => ({ error }),
  );

  while (true) {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const outcome = await Promise.race([
      done,
      new Promise<null>((resolve) => {
        timer = setTimeout(() => resolve(null), HEARTBEAT_MS);
      }),
    ]);
    clearTimeout(timer);
    if (signal.aborted) {
      return;
    }
    if (outcome === null) {
      yield { type: "raw", rawValue: "heartbeat" };
      continue;
    }
    if ("error" in outcome) {
      throw outcome.error instanceof Error
        ? outcome.error
        : new Error(String(outcome.error));
    }
    if (outcome.result.status === "error") {
      throw new Error(outcome.result.error);
    }
    yield {
      type: "text-delta",
      id: "enhance-local",
      text: outcome.result.data.text,
    };
    return;
  }
}

/**
 * After `enhance_local` saved the note: generate a title when the session has
 * none (the title task persists it) and run the usual completion side effects.
 */
export const runLocalEnhanceSuccess: NonNullable<
  TaskConfig<"enhance">["onSuccess"]
> = async ({ text, args, model, startTask, getTaskState, signal }) => {
  if (signal.aborted) {
    return;
  }
  const snapshot = await loadSessionContentSnapshot(args.sessionId);
  if (!snapshot) {
    throw new Error(`Session ${args.sessionId} no longer exists`);
  }

  if (
    isGenericSessionTitle(snapshot.title) &&
    !hasLiveSessionTitleDraft(args.sessionId) &&
    text.trim()
  ) {
    await generateTitle({ args, text, model, startTask, getTaskState });
  }
  if (signal.aborted) {
    return;
  }

  notifyEnhanced(args.sessionId);
};

async function generateTitle(params: {
  args: { sessionId: string };
  text: string;
  model: LanguageModel;
  startTask: Parameters<
    NonNullable<TaskConfig<"enhance">["onSuccess"]>
  >[0]["startTask"];
  getTaskState: Parameters<
    NonNullable<TaskConfig<"enhance">["onSuccess"]>
  >[0]["getTaskState"];
}) {
  const { args, text, model, startTask, getTaskState } = params;
  const titleTaskId = createTaskId(args.sessionId, "title");
  if (getTaskState(titleTaskId)?.status === "generating") {
    return;
  }
  try {
    await startTask(titleTaskId, {
      model,
      taskType: "title",
      args: { sessionId: args.sessionId, enhancedNote: text },
    });
  } catch (error) {
    // A missing title must not fail an already-saved enhancement.
    console.error("[enhance-local] title generation failed", error);
  }
}

function notifyEnhanced(sessionId: string) {
  void localApiCommands.dispatchEvent("note.enhanced", sessionId);
  void runNoteEnhancedAutomations(sessionId);
  syncCloudApiSnapshotBestEffort(sessionId);
  void playCompletionSound();
  void requestAppAttention();
}
