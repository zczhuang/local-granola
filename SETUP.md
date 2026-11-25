# Local Granola: Mac setup

This is a private, personal fork of [fastrepl/anarlog](https://github.com/fastrepl/anarlog) (MIT, see `LICENSE`). It changes anarlog into a Granola-style notetaker that runs fully on one Apple Silicon Mac. Meeting audio, transcripts and notes stay on the Mac. Notes are written by a local Ollama model.

Target machine: M5 Pro with 36 GB of memory.

What the fork adds, in four phases plus MCP:

1. **Local lockdown.** The fork is built with `LOCAL_ONLY` on (Rust `ANARLOG_LOCAL_ONLY`, TS `VITE_LOCAL_ONLY`; both default on).
   - Off and locked: telemetry, crash reports, cloud sync, the updater, hosted LLM and STT providers, sign-in and the cloud API.
   - Web requests are limited to localhost.
   - Audio is deleted after transcription by default. You can keep it for N days instead in Settings > Audio.
2. **Transcription.** Live transcription uses Parakeet v3 on the Neural Engine. After about 30 seconds of speech, a language check decides whether the meeting is Mandarin or mixed. If it is, a Whisper large-v3-turbo pass reruns the whole meeting afterwards, in the right language (settings "Whisper pass after meetings": off, auto or always, and "Language routing": auto, Parakeet or Whisper). Your custom vocabulary corrects terms in the live transcript.
3. **Speakers.** Zoom and Teams "who is speaking" cues are read from the Accessibility tree and combined with voiceprints and calendar attendees.
4. **Enhanced notes.** Your typed lines are the backbone, kept verbatim and in order.
   - AI bullets (gray) go under your lines. Each cites the transcript, and a click jumps to the exact spot.
   - Re-enhancing keeps your edits and honors your deletes. Regenerate starts over, but still keeps your typed lines.
   - The notes model is `gemma4:26b-mlx` through Ollama. `qwen3.6:35b-mlx` is an optional Mandarin trial.
5. **MCP for Claude and Codex.** `anarlog mcp` is a stdio server, read-only by default. See `docs-local/MCP.md`.

Code written on Linux that could not be built or run there is marked `// LOCAL-GRANOLA(mac-untested):`. Find it with `rg 'LOCAL-GRANOLA\(mac-untested\)'`.

## 0. Before you start

- Xcode, the full app, with its Metal toolchain: `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcodebuild -downloadComponent MetalToolchain`.
- Node 22+, pnpm 11.1.1, Rust 1.94.0 (rustup reads `rust-toolchain.toml`), `brew install process-compose`.
- Ollama for macOS, running (`ollama serve` or the menu bar app).
- **Spaces in the path.** The clone folder below has spaces in it (`vibe coding projects`), and some native dependencies refuse build directories with spaces. Always build with a target dir without spaces:

  ```sh
  export CARGO_TARGET_DIR="$HOME/.cargo-target/local-granola"
  ```

## 1. Clone (Claude Code session on the MacBook Pro)

Run these in the empty `granola-local` folder under `~/vibe coding projects`. The folder already has an empty `git init`, so remove that first:

```sh
cd ~/"vibe coding projects/granola-local"
rm -rf .git                      # only if the folder is still empty apart from .git
git clone https://github.com/zczhuang/local-granola.git .
git switch local-granola         # if it isn't the default branch
```

Then work through sections 2 to 7 in order. Stop and report at the first step that fails, with the full error.

## 2. Regenerate the lock file and install

The workspace was edited on Linux, where two dependencies could not be fetched (whisper-rs from codeberg.org and ONNX runtime from cdn.pyke.io). So `Cargo.lock` was edited by hand and **must** be regenerated here, without `--locked`:

```sh
pnpm install
cargo metadata --format-version 1 > /dev/null   # rewrites Cargo.lock
git diff --stat Cargo.lock                      # expect changes for stt-routing, transcript, local-only, enhanced-notes and friends
```

Commit the regenerated `Cargo.lock` as its own commit.

## 3. Build and test what Linux couldn't

```sh
# Rust: crates and plugins the fork changed
cargo test -p local-only -p audio-retention -p db-app -p stt-routing -p transcript \
  -p voiceprint -p language -p enhanced-notes -p agent-access -p anarlog-cli \
  -p tauri-plugin-template -p tauri-plugin-transcription
cargo test -p transcribe-soniqo -p transcribe-whisper-local -p whisper-local -p listener-core
cargo update -w                    # settles the whisper-rs entries that couldn't resolve on Linux
cargo test -p audio-actual          # Zoom and Teams speaking-cue dispatch
cargo check -p tauri-plugin-anlg-tray
cargo check -p desktop              # the Tauri app crate (startup changes in lib.rs and db.rs)

# Swift: meeting capture, Zoom and Teams adapters
swift test --package-path tools/meeting-capture

# TypeScript
pnpm -F @anlg/ui build
pnpm -F @anlg/desktop typecheck
pnpm -F @anlg/desktop test
pnpm -F @anlg/editor test
```

If a mac-untested spot fails to compile, fix it in place, keep the marker comment, and say what you changed.

## 4. Pull the models

```sh
ollama pull gemma4:26b-mlx          # notes model, about 17 GB
ollama pull qwen3.6:35b-mlx         # optional: Mandarin trial after meetings
```

Tip: `launchctl setenv OLLAMA_KEEP_ALIVE 30m` (then restart Ollama) keeps the notes model loaded between meetings, so enhancing starts quickly. With 36 GB, don't keep both models loaded at once.

Parakeet v3 and Whisper large-v3-turbo download from inside the app (Settings > Transcription). These downloads are the main network traffic left; see section 8.

## 5. Run the app

```sh
pnpm dev:desktop
```

Then:

1. Grant microphone, screen and system audio recording, and Accessibility (Accessibility is needed for the Zoom and Teams speaker cues).
2. Settings > Transcription: download Parakeet v3 and Whisper large-v3-turbo. Pick Parakeet v3 Live as the live model. Leave "Whisper pass after meetings" and "Language routing" on Auto.
3. Settings > Intelligence: choose Ollama, model `gemma4:26b-mlx`.
4. Settings > Audio: leave "Delete audio after transcription" on, or set a number of days.

## 6. Verify on real meetings

Do each and note what you saw:

1. **No traffic off the Mac.** Run Little Snitch or `nettop -p <pid>` during a meeting. The only connections should be to localhost (Ollama, the local STT server). Model downloads in step 5 are the exception.
2. **English meeting.** Live transcript appears. Typed notes stay as typed. Enhance adds gray bullets under your lines. Clicking a bullet jumps to the transcript and highlights the words for a few seconds.
3. **Edit and re-enhance.** Edit one AI bullet and delete another, then type a new line. About 20 seconds later the notes re-enhance: your edit stays, the deleted bullet stays gone, and the new line gets bullets. Regenerate resets the AI bullets but keeps your lines.
4. **Mandarin or mixed meeting.** After it ends, the Whisper re-pass runs (status shows processing), and the transcript is replaced with the better one. The audio is deleted only after the re-pass finishes (default retention).
5. **Zoom and Teams.** In a 3+ person call, speakers get names. The Accessibility labels and the lag constants were assumed, not seen live, so:
   - Re-record the AX fixtures used by `SpeakingSignalTests.swift` from a real Zoom and a real Teams call.
   - Measure how far the AX "speaking" cue lags the audio, and adjust `ZOOM_CUE_DELAY_MS` (1200) and `TEAMS_CUE_DELAY_MS` (1500) in `crates/transcript/src/meeting_speakers.rs`.
6. **Audio retention.** Set 1 day, record a short meeting, and confirm the file is gone after a day. Set it back.

## 7. Connect Claude Code and Codex (MCP)

Full details are in `docs-local/MCP.md`. Short version:

```sh
cargo build --release -p anarlog-cli
BIN="$CARGO_TARGET_DIR/release/anarlog"

# Claude Code
claude mcp add --scope user local-granola -- "$BIN" mcp
```

Codex, in `~/.codex/config.toml` (put the absolute binary path in place of `$BIN`):

```toml
[mcp_servers.local-granola]
command = "/Users/<you>/.cargo-target/local-granola/release/anarlog"
args = ["mcp"]
```

The server reads `~/Library/Application Support/anarlog/app.db` read-only. Add `--allow-writes` to the args only if you want the upstream write tools. Smoke test: ask Claude or Codex to search your meetings, then to get the enhanced notes for a hit and quote one bullet's transcript source.

## 8. What can still reach the network

- Model downloads you start: `models.anarlog.so` (Parakeet, Whisper) and Hugging Face.
- App assets from `static.anarlog.so` (icons and images in some screens).
- Calendar sync, if you connect a calendar (Apple Calendar is local; Google or Outlook are not).
- Ollama itself, only if you point it at a remote model (don't).

Everything else is off at build time.

## 9. Known gaps

- Not run on a Mac at all yet: the Tauri app, the Swift code, the soniqo live bridge, Whisper language detection, and the Zoom and Teams cues.
- The app has no Content Security Policy in `tauri.conf.json`. Web requests are still limited to localhost by the Tauri http scope.
- Cancelling an enhance in the UI doesn't stop the model call already running; it finishes and is discarded.
- Summaries made before this fork (or by the hosted path) are left alone. A plain Enhance on one shows an error; use Regenerate to replace it with enhanced notes.
- `apps/desktop-gpui` (the experimental GPUI app) still has the old audio-retention rule. This fork uses the Tauri app only.
- `bindings.gen.ts` for the template and transcription plugins was edited by hand. After the first `pnpm dev:desktop`, check `git diff` on those files against the regenerated output.
