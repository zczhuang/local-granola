# Oatmeal Local MCP Server (Claude Code, Codex, Antigravity, Claude Desktop)

`oatmeal-mcp` (built from `apps/cli` / `anarlog-cli`) is a stdio MCP server over your local Oatmeal meeting database. Coding agents launch it as a child process and communicate over stdin/stdout. It opens **no network listener**, makes **no cloud calls**, and sends **no analytics** (`LOCAL_ONLY`). The only network request it can ever make is an optional call to a **loopback** Ollama instance for `preview_enhanced_notes`.

By default the database is opened **read-only**. Pass `--allow-writes` only if you want to enable proposal and recording-import write tools; without it, nothing is written.

## 1. Build & install (macOS)

```sh
export CARGO_TARGET_DIR="$HOME/.cargo-target/oatmeal"
cargo build --release -p anarlog-cli

mkdir -p ~/.local/bin
rm -f ~/.local/bin/oatmeal-mcp
cp "$CARGO_TARGET_DIR/release/anarlog" ~/.local/bin/oatmeal-mcp
chmod +x ~/.local/bin/oatmeal-mcp
codesign --force --sign - ~/.local/bin/oatmeal-mcp
```

> Note: Always `rm -f` and re-sign (`codesign --force --sign -`) when replacing the binary on Apple Silicon so macOS doesn't SIGKILL a stale code-signature cache.

## 2. Database path

Default on macOS: `~/Library/Application Support/anarlog/app.db`.

Override with `--db-path /path/to/app.db`, the `ANARLOG_DB_PATH` environment variable, or `--base <data dir>`.

Ollama settings (used only by `preview_enhanced_notes`): `ANARLOG_OLLAMA_URL` (or `OLLAMA_HOST`) and `ANARLOG_OLLAMA_MODEL`. Non-loopback URLs are refused.

## 3. Register the server

Replace `<you>` with your macOS username in the absolute paths below.

### Claude Code

```sh
claude mcp add --scope user oatmeal -- /Users/<you>/.local/bin/oatmeal-mcp \
  --db-path "/Users/<you>/Library/Application Support/anarlog/app.db" mcp
```

### Codex

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.oatmeal]
command = "/Users/<you>/.local/bin/oatmeal-mcp"
args = ["--db-path", "/Users/<you>/Library/Application Support/anarlog/app.db", "mcp"]
```

### Antigravity

Add to `~/.gemini/config/mcp_config.json` under `mcpServers`:

```json
{
  "mcpServers": {
    "oatmeal": {
      "command": "/Users/<you>/.local/bin/oatmeal-mcp",
      "args": [
        "--db-path",
        "/Users/<you>/Library/Application Support/anarlog/app.db",
        "mcp"
      ]
    }
  }
}
```

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`, then restart Claude Desktop:

```json
{
  "mcpServers": {
    "oatmeal": {
      "command": "/Users/<you>/.local/bin/oatmeal-mcp",
      "args": [
        "--db-path",
        "/Users/<you>/Library/Application Support/anarlog/app.db",
        "mcp"
      ]
    }
  }
}
```

Add `"--allow-writes"` to `args` only if you want the write tools.

## 4. Tools

Everything below runs locally against your SQLite database. Only `preview_enhanced_notes` calls a model, and only on loopback Ollama.

| Tool | Reads / does | Parameters |
| --- | --- | --- |
| `search_meetings` | Searches titles, typed notes, enhanced notes, and transcripts. Paginated, with snippets and `mm:ss` timestamps. Read-only. | `query`, `folder_path`, `limit`, `offset` |
| `get_enhanced_notes` | Returns the enhanced notes of a meeting: your lines, AI bullets with resolved transcript citations, decisions, action items, open questions, and Markdown footnotes. Read-only. | `meeting_id`, `document_id` |
| `get_transcript_segment` | Returns transcript text by source reference (`transcript_id` + word IDs) or time range (`start_ms`, `end_ms`). Read-only. | `meeting_id`, `transcript_id`, `start_word_id`, `end_word_id`, `start_ms`, `end_ms`, `max_words` |
| `preview_enhanced_notes` | **Calls local Ollama.** Generates enhanced notes and returns them **without saving**. | `meeting_id`, `model` |
| `list_meetings` / `get_meeting` / `get_meeting_transcript` | Upstream Anarlog read tools for browsing and reading meeting transcripts. | See tool schemas |

## 5. Smoke test

1. Confirm registration with `claude mcp list` or `codex mcp list`.
2. Ask your agent: *"Use oatmeal to search my meetings for `<a recent topic>`."*
3. Ask: *"Get the enhanced notes for the first match and quote the transcript citation for one action item."*
