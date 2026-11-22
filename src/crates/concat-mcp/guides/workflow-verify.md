---
name: eloncut-verification
description: Verify a Concat MCP timeline edit with current project state and fresh composed frames before reporting success. Use after trimming, cropping, positioning, adding text, or otherwise changing a visible cut.
license: Apache-2.0
metadata:
  upstream: ChatCut-Inc/agent-plugin
  upstream-revision: a69a54fac266c69a9e76d2767af2e82776a17464
---

# Verify a Concat edit

Verify timeline state and fresh composited frames before reporting a visible edit.

Adapted for MindCut from ChatCut-Inc's [Codex Verification skill](https://github.com/ChatCut-Inc/agent-plugin/blob/a69a54fac266c69a9e76d2767af2e82776a17464/codex/skills/verification/SKILL.md). Upstream supplies no separate per-file copyright header. This file is modified; the upstream Apache-2.0 license is preserved in [LICENSE](../LICENSE).

1. Confirm the open project directory and the edit the user requested. Read the active timeline with `timeline_read`; use `project_get` if the summary does not show the settings you need. Check identities, tracks, times and expected settings against the request, not against recollection.
2. For a visible change, call `preview_frame` with `path` and `time` in timeline seconds. Leave `output` unset so the tool returns the image without writing a file. Sample within the affected interval, at meaningful boundaries on both sides of a cut, and outside the range when neighboring material should be unchanged. Stay before the timeline's end; for exact boundary tests use adjacent renderable frames. Inspect the actual returned image for composition, text legibility and unintended changes.
3. Report what was checked and at which timecodes. Separate a structural check from a visual check; a successful response without inspectable pixels is not visual evidence. Sampled stills are not proof of continuous playback, pacing, sound, or lip sync.

If frames fail, record the failure and summarize only what the state proves. If audio or motion is material to acceptance, ask the user to play the relevant interval and report it as unverified until then. Do not manufacture an alternate render result or export a file just to self-check. This skill does not save, mutate or export the project.

## What you cannot do

Timeline state and sampled stills do not verify audible words, motion or
continuous playback. Automatic transcription is unavailable through MCP.
There is no dedicated MCP tool for voice isolation, but `edit_apply` with
`setClipVoice` sets a clip's `speech` or `vocals` mix (see the index for an
example). Reading it back does not verify model availability, rendered
separation, or how it sounds. Ask the user to play the affected interval in
the app if confirmation is needed. Tell the user what could not be verified;
do not fabricate evidence. See the index for other unavailable features.

In the source repository, the preserved license is in
[docs/chatcut-skill-trial/LICENSE](../../../../docs/chatcut-skill-trial/LICENSE).
