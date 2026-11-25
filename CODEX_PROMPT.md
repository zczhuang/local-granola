You are working in my private fork of anarlog, "local-granola", on my Mac (Apple Silicon, M5 Pro, 36 GB). It was built and unit-tested on Linux, so the Mac-only parts have never been compiled or run.

Read SETUP.md and AGENTS.md first, then do SETUP.md sections 2, 3 and 7 in order:

1. Export CARGO_TARGET_DIR to a path without spaces (the checkout path has spaces).
2. Regenerate Cargo.lock without --locked, and commit it on its own.
3. Run every build and test command in section 3. For each failure in code marked `LOCAL-GRANOLA(mac-untested)`, make the smallest fix, keep the marker, and rerun. Don't skip or weaken tests, and don't touch anything under enterprise/.
4. Build anarlog-cli in release and print the exact `claude mcp add` command and the ~/.codex/config.toml block for this machine, with the real absolute path.

Don't change any setting on this Mac, install anything system-wide, or push. Commit fixes locally on the local-granola branch, one commit per fix.

Finish with a short report: each command with pass or fail, each fix (file and why), and anything still failing with its full error.

Sections 4 to 6 (pulling models, running the app, live meetings) are mine to do.
