# Local MCP server (Claude Code, Claude desktop, Codex)

`anarlog mcp` is a stdio MCP server over your local meeting database. Claude Code, Claude desktop and Codex launch it as a child process and talk to it over stdin/stdout. It opens **no network listener**, makes **no cloud calls** and sends **no analytics** (this fork is built with `LOCAL_ONLY`). The only network traffic it can ever produce is the optional call to a **loopback** Ollama used by `preview_enhanced_notes`.

By default the database is opened **read-only**. Pass `--allow-writes` (`anarlog mcp --allow-writes`) to enable the upstream proposal and recording-import tools; without it they return an error and nothing is written.

## 1. Build (Mac)

```sh
cargo build --release -p anarlog-cli
# binary: target/release/anarlog
```

Use the absolute path of that binary in every snippet below, for example `/Users/you/lg/target/release/anarlog`.

## 2. Database path

Default on macOS: `~/Library/Application Support/anarlog/app.db`.

Override with `--db-path /path/to/app.db`, the `ANARLOG_DB_PATH` environment variable, or `--base <data dir>`.

Ollama settings (only used by `preview_enhanced_notes`): `ANARLOG_OLLAMA_URL` (or `OLLAMA_HOST`) and `ANARLOG_OLLAMA_MODEL`. The URL must be loopback (`localhost`, `127.0.0.1`, `::1`); anything else is refused.

## 3. Register the server

### Claude Code

```sh
claude mcp add --scope user local-granola -- /absolute/path/to/anarlog mcp
```

### Claude desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`, then restart the app:

```json
{
  "mcpServers": {
    "local-granola": {
      "command": "/absolute/path/to/anarlog",
      "args": ["mcp"]
    }
  }
}
```

### Codex

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.local-granola]
command = "/absolute/path/to/anarlog"
args = ["mcp"]
```

Add `"--allow-writes"` to the args only if you want the write tools.

## 4. Tools added by this fork

Everything below runs locally against your database. Only `preview_enhanced_notes` calls a model, and only a local Ollama.

| Tool | Reads / does | Parameters |
| --- | --- | --- |
| `search_meetings` | Searches titles, typed notes, enhanced notes and transcripts. Paginated, with snippets and `mm:ss` times. Read-only. | `query`, `folder_path`, `limit`, `offset` |
| `get_enhanced_notes` | Returns the enhanced notes of a meeting: your lines, the AI bullets under them with resolved transcript sources, decisions, action items (owner and due when known), open questions, and Markdown with footnote citations like `[^1]: [00:00–00:01]`. Read-only. | `meeting_id`, `document_id` |
| `get_transcript_segment` | Returns transcript text by source reference (`transcript_id` with `start_word_id` and `end_word_id`) or by time range (`start_ms`, `end_ms`). Read-only. | `meeting_id`, `transcript_id`, `start_word_id`, `end_word_id`, `start_ms`, `end_ms`, `max_words` |
| `preview_enhanced_notes` | **Calls the local model.** Generates enhanced notes with a loopback Ollama and returns them **without saving**. Never applied automatically. Needs the `ollama` feature (on by default). | `meeting_id`, `model` |

Upstream tools (`list_meetings`, `get_meeting`, and so on) are described in the main MCP docs.

## 5. Smoke test

1. Register the server (section 3) and start a new Claude Code or Codex session. Confirm it is listed with `claude mcp list` or `codex mcp list`.
2. Ask: "Use local-granola to search my meetings for `<a word you said recently>`." You should get hits with titles, dates and snippets.
3. Ask: "Get the enhanced notes for the first hit and quote the transcript source of one bullet." Then, with Ollama running (`ollama serve`), ask for a `preview_enhanced_notes` of that meeting and confirm nothing was saved (the meeting looks the same in the app).
