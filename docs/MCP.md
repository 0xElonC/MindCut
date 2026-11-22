# Concat for AI agents (MCP)

Concat ships an [MCP](https://modelcontextprotocol.io) server inside the app. An agent - Claude Code, Claude Desktop, or any other MCP client - starts it as `concat --mcp` and can then create projects, import media, cut, add titles and captions, keyframe, apply effects, look at frames and export, with the same edit commands the window uses. Nothing extra to install.

## Set it up

Point your client at the Concat binary with the one argument `--mcp`.

| System | Command |
| --- | --- |
| macOS | `/Applications/Concat.app/Contents/MacOS/concat` |
| Windows (installed for you) | `%LOCALAPPDATA%\Programs\Concat\concat.exe` |
| Windows (installed for everyone, or the .msi) | `C:\Program Files\Concat\concat.exe` |
| Linux (.deb, .rpm, Arch) | `/usr/bin/concat` |
| Linux (AppImage) | the `.AppImage` file itself |

**Claude Code:**

```sh
claude mcp add concat -- /Applications/Concat.app/Contents/MacOS/concat --mcp
```

**Claude Desktop** (Settings > Developer > Edit Config, `claude_desktop_config.json`), and most other clients, which take the same shape:

```json
{
  "mcpServers": {
    "concat": {
      "command": "/Applications/Concat.app/Contents/MacOS/concat",
      "args": ["--mcp"]
    }
  }
}
```

On Windows, write the path with doubled backslashes in JSON: `"C:\\Program Files\\Concat\\concat.exe"`.

## How it reaches Concat

`concat --mcp` works one of two ways, and the `concat_version` tool says which (`mcp.mode`):

- **Through the window** (`window`). When Concat is open with **Settings > Automation > Scripts and apps** on, the window writes where its server is, and its token, to `automation.json` in Concat's config folder (readable only by you). The MCP server finds it and goes through the window's server. This is the safer way while you are editing: the agent cannot open the project you have on screen and save over it, and exports take turns with yours.
- **On its own** (`headless`). Otherwise the MCP server runs Concat's engine itself, with no window. Nothing has to be open. Every Concat process locks the project folders it has open (`concat.lock`, let go on close or when the process dies), so a project open in the window is refused here with a note to turn on Automation and use `--mcp window`, and the window refuses a project an agent has open.

Choose explicitly with `--mcp window` (fail rather than run on its own) or `--mcp headless`; the default is `--mcp auto`. To reach a server elsewhere, set `CONCAT_API_ADDRESS` (e.g. `127.0.0.1:7420`) and `CONCAT_API_TOKEN` in the client's `env` for the server.

Either way the agent writes files - projects, exports, frames - only under your home folder. Set `CONCAT_MCP_ROOTS` (a path list, `:`-separated, `;` on Windows) to allow other folders in headless mode.

The built-in Agent pane supports **Live mode**, enabled by default. With it on, the agent can open the project on screen and edit it directly: changes appear in the window and join its Undo history. Toggle Live mode in the Agent pane; with it off, the agent must work on a separate project or a copy. The built-in agent uses its own window API bridge; connecting an external MCP client through Settings > Automation does not by itself enable live editing of the project on screen.

## What the agent can do

Tools are named for what they do. Editing tools derive their argument schemas from the Concat API's own types, so they carry the same limits and descriptions the engine does; `guide_read` reads the manual locally in the MCP server.

| Area | Tools |
| --- | --- |
| Operation manual | `guide_read` (read-only; omit arguments for topic ids and summaries, or pass `topic` for one article; no open project required) |
| Projects | `project_list`, `project_create`, `project_open`, `project_save`, `project_close`, `project_set_video` |
| Reading | `timeline_read` (compact: lanes, clips in time order, media, ids), `project_get` (everything), `concat_version` |
| Media | `media_probe`, `media_import` |
| Editing | `clip_add`, `clips_add_sequence`, `clip_trim`, `clips_split`, `clips_move`, `clips_remove`, `clip_update`, `clip_transform`, `clip_speed`, `clip_frame`, `track_add`, `timeline_linkage` |
| Picture | `clip_masks`, `mask_key_set`, `clip_stabilize`, `timeline_background` |
| Titles and captions | `title_add`, `title_style_words` (some words in another colour, weight, size...), `title_clear_word_styles`, `captions_add` |
| Keyframes and effects | `keyframe_set`, `keyframe_clear`, `effects_list`, `effect_layer_add`, `effect_key_set` |
| Anything else | `edit_apply` (any edit command, or a batch of them as one undo step), `edit_undo`, `edit_redo` |
| Templates | `template_list`, `template_instantiate` |
| Local AI status | `ai_status` (model installation and activity) |
| Transcription | `media_transcribe`, `transcript_query` (text and word timings in source seconds) |
| Speech generation | `speech_voices`, `speech_generate` (local WAV imported into the bin) |
| Vocal separation | `audio_separate_vocals` (creates the cache before `edit_apply` with `setClipVoice`) |
| Picture enhancement | `clip_enhance` (replaces the clip source after an enhanced cache copy succeeds) |
| Export and job control | `export_start`, `export_status`, `export_wait` (export progress), `export_cancel` |
| Looking | `preview_frame` (the composited frame as an image) |

Edits stay in memory until `project_save`, as in the window. Times are seconds; a keyframe's position is a fraction of its clip (0 to 1); sizes and offsets are fractions of the frame.

Before editing, read the manual directory with `guide_read`, then read the relevant topics. The manual is embedded in the MCP binary at compile time and returned as text in both window and headless modes; no skill files are installed in your home folder or project.

For local AI workflows, call `guide_read` with `{"topic":"ai-tools"}`. Check
`ai_status` first, then poll background jobs with `export_status` and use their
results once the state is `done`. The tools never download models. Install
missing models in Settings > Transcriber or Settings > Speech, or use Voice
Isolation > Song (Spleeter) or Enhance > Preview (Real-ESRGAN) in the clip
inspector.

## Protocol

MCP over stdio, newline-delimited JSON-RPC 2.0. Both protocol eras are served: the `initialize` handshake (2025-11-25, 2025-06-18, 2025-03-26, 2024-11-05) and the per-request `_meta` of 2026-07-28 with `server/discover`. `ping`, `tools/list`, `tools/call`, progress notifications for `export_wait`, and `notifications/cancelled` are supported. Standard output carries only protocol; the server logs to standard error.

For scripts that do not speak MCP, the same API is served as JSON-RPC lines on a socket from Settings > Automation; see [the developer docs](https://concat.standalones.org/docs).
