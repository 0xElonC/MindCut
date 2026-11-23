# Agent backends

`concat-agent` exposes `backend::Backend` for starting and resuming a session, sending text and images, interrupting a turn, listing models and effort levels, and receiving `Event::Update(wire::Update)`. `Event::Closed` reports loss of the persistent Codex connection. The callbacks may run immediately or on a background thread; the window must dispatch them to its UI thread. Calls to `ClaudeBackend::status` launch a local status subprocess and belong off the UI thread.

- **Codex:** `CodexBackend::connect` starts the existing app-server, does the same handshake, and responds to approval requests as before. The same wire functions encode its conversation and turn requests and decode its history, notifications, skills and model settings. `SessionSetup::from` preserves the existing MCP server name when migrating a Codex caller.
- **Claude Code:** Construct `ClaudeBackend::new` with the executable from `locate::claude()`, an application-private history directory, and an event callback. The locator looks for `CONCAT_CLAUDE`, then the executable on `PATH` and in standard native, npm and Homebrew install directories. A turn runs a short-lived Claude Code process in the provided project directory. Each turn sends one JSON line on stdin and closes it. It carries a `user` message with text blocks and user-selected PNG/JPEG/GIF/WebP attachments as base64 image blocks (5 MiB per raw file); the pathname is not sent to the model. Codex skills do not carry over.
- **Antigravity:** `acp::connect` runs Google's `agy acp` (found by `antigravity::locate()`, or the program named in `CONCAT_AGY`) and speaks the Agent Client Protocol: JSON-RPC lines over its standard input and output, through `peer::Peer`. `antigravity::PROFILE` holds what is Antigravity's own (its arguments, where the instructions go in a session's `_meta`, that only its Gemini models are offered, and how its MCP tool names are shown); `acp` names no product. The program signs in by itself, so the app never asks for a key. It lets the agent run what it asks, as Codex does. Skills and renaming do not apply. The program does not give a conversation's history back, so the backend keeps its own record in `agent-antigravity/` under the settings folder (`transcript::Transcripts`, one JSON line per item, written when the person speaks and when a turn ends) and returns it from `resume_session`. Deleting a conversation deletes that record.

The chat index writes `provider: "codex"`, `"claude"` or `"antigravity"`. Missing `provider` in an older record means Codex. The controller can use `Store::put_with_provider` to save a session and `Store::provider` to choose the backend before opening it.

## Claude Code launch policy

| Arguments | Why |
| --- | --- |
| `-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages` | Run without a terminal, exchange one JSON object per line and receive text/thinking deltas before the result. |
| `--tools ""` | Do not expose built-in tools such as Bash or filesystem readers/writers. |
| `--strict-mcp-config --mcp-config <json>` | Use only the configured MCP server, with its exact command, arguments and environment from `NamedMcpServer`. |
| `--allowedTools mcp__<server-name>__* --permission-mode dontAsk` | Allow only tools of that MCP server; deny tool permission requests that would otherwise wait for confirmation. The prefix uses the configured name, not a product name. |
| `--setting-sources "" --settings '{"disableAllHooks":true}' --disable-slash-commands` | Do not load ordinary user/project/local settings, run configured hooks, or advertise skills and slash commands in app turns. Admin-managed CLI policy is outside the adapter's control. |
| `--system-prompt <base> --append-system-prompt <instructions> --system-prompt-snapshot off` | Use a minimal base prompt, then the caller's current project instructions; refresh those instructions each turn rather than reusing a saved prompt. |
| `--model=<id> --effort=<level>` | Apply the current turn's choices when provided; otherwise let the CLI choose. The picker offers the Claude CLI aliases `sonnet`, `opus`, `haiku`, and advertises `low`/`medium`/`high` for Sonnet and Opus. This is not an account-specific catalog. |
| `--resume <id>` | On later turns, resume the saved Claude Code session explicitly. Do not use `--continue` (which could select another conversation) or `--no-session-persistence` (which would prevent resume). |

The app stores rendered items and the non-credential session ID in its own history directory, independently of Claude Code's transcript. It restores the display from its own history and does not open provider-private files. Claude Code retains the actual context used by `--resume`; if that context is removed or the project path changes, the app can still display prior messages but cannot reconstruct Claude's context. Deleting the app history does not delete Claude Code's transcript. An interrupted turn can leave partial messages in both histories; stopping kills the CLI process, not already-completed MCP actions. Image previews after restart still depend on the original local files.

## Stream-json mapping

| Claude Code event or local transition | `Update` behavior |
| --- | --- |
| Local turn accepted; `system/init` | Emit `TurnStarted` once for the local turn; take the CLI session ID from `init` or `result`. |
| First local user text | Emit `Named` using a short excerpt; the observed print stream does not guarantee a generated title. |
| `stream_event.content_block_start` with `text` or `thinking` | Emit `ItemStarted` and track the block index for the current assistant message. |
| `stream_event.content_block_delta` with `text_delta` or `thinking_delta` | Emit `AgentWords` or `ThoughtWords`, respectively. |
| `stream_event.content_block_stop` | Emit `ItemDone` with the accumulated text or thought; for a tool, wait for its result. |
| `assistant.message.content` | Emit complete text/thought items if no partial stream was received for that message; do not duplicate streamed content. Recognize full `tool_use` blocks as well. |
| `tool_use` / `input_json_delta` | Emit one running tool item, accumulate its arguments and match later results using the tool-use ID. |
| `user.message.content[].tool_result` | Emit `ItemDone` with output and `Done` or `Failed`; a tool still running when the turn exits is marked failed. |
| `assistant.error` or protocol `error` | Emit `Trouble` using a fixed diagnostic rather than raw stderr or error payloads. |
| `result` and process exit | Emit `TurnEnded` with `completed`, `failed`, or `interrupted`; a missing result is a failure unless the turn was stopped locally. `is_error: true` is a failure even when `subtype` is `success`. |
| Unknown events, unsupported block types, malformed JSON | Ignore. Thinking is only visible if Claude returns thinking text. |

## Authentication and verification

Claude Code owns login and authentication. The adapter reads the `loggedIn` boolean and exit code from `claude auth status`, never opens a credential store, and does not persist or forward auth status output. It launches with a small allowlist of OS and configuration-path environment variables rather than inheriting token variables or setting `ANTHROPIC_API_KEY`; authentication that depends on an environment-provided credential is not supported. The adapter does not log raw stdout/stderr. The settings above restrict which tools can run, but are not an operating-system sandbox: Claude Code still runs in the supplied working directory and can construct its own context or apply managed policy. Users should confirm the applicable Anthropic terms before using their Claude subscription through this integration.

Observed with Claude Code `2.1.330` on macOS: normal status returned `{"loggedIn":true}` (exit `0`, showing the projected field only) using the adapter's environment allowlist. Running `CLAUDE_CONFIG_DIR=<empty temp directory> claude auth status` returned `{"loggedIn":false}` (exit `1`); the temporary directory was removed without signing out the real account. A missing executable is `NotInstalled`. An unavailable or unsupported status command is reported separately from a confirmed signed-out state.

Two manual `claude -p` probes used short text and no tools, without saving probe sessions. The first returned the partial-text ordering used in `src/crates/concat-agent/src/claude/fixtures/turn.jsonl` (sanitized IDs and text; thinking/tool cases are synthetic). A second, using a narrower environment than the adapter, emitted an empty tool list, `Not logged in · Please run /login`, then `is_error: true` and exited `1`. The CLI's total estimated cost was `$0.0119929`. Automated tests use fixture data and a local fake executable only; they do not use a real Claude account, network, or live MCP server. Windows CLI launching and real MCP permission enforcement were not exercised in this verification.
