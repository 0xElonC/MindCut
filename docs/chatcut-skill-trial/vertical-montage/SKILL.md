---
name: eloncut-vertical-montage
description: Assemble locally available footage into an editable vertical short with a clear opening and intentional shot order. Use for a Reel, Short, or 9:16 montage from multiple clips, not for automatic highlight detection or generating footage.
license: Apache-2.0
metadata:
  upstream: ChatCut-Inc/agent-plugin
  upstream-revision: a69a54fac266c69a9e76d2767af2e82776a17464
---

# Assemble a vertical montage in Concat

Adapted for MindCut from ChatCut-Inc's [Kimi Work Editing Guide](https://github.com/ChatCut-Inc/agent-plugin/blob/a69a54fac266c69a9e76d2767af2e82776a17464/kimi/skills/chatcut-editing-guide/SKILL.md). Upstream supplies no separate per-file copyright header. This file is modified; the upstream Apache-2.0 license is preserved in [LICENSE](../LICENSE).

1. Confirm the target length, what should hook the viewer, and the project path. For an existing project, use `project_open` if it is not already open, then read the active timeline with `timeline_read`. If it already contains edits, don't change its canvas or overwrite it without permission: offer a separate project, and use `project_create` only after the user agrees. If there is no named project or absolute local media path, ask rather than guessing.
2. Use `media_probe` to confirm duration and dimensions for each local file, then `media_import` to add it to the agreed project. Get media IDs from each import or `timeline_read`. Make a shot plan: source, source in/out, timeline order, and what each shot contributes. Start with the strongest evidence or action, then provide context and a payoff. Omit redundant shots. Use the user's picks or timecodes if you cannot see or hear the source reliably; do not invent content or timestamps.
3. On the agreed timeline, use `project_set_video` for the requested vertical frame (for example, 1080 × 1920), preserving the source/project frame rate when appropriate. If the plan uses complete files, lay them end-to-end with `clips_add_sequence`. For selected ranges, work one clip at a time: place it with `clip_add` in an empty span on the intended track, trim its edges with `clip_trim` in timeline seconds, then move the validated trim to its planned start with `clips_move`. Read back timing and check for overlaps; do not ripple a populated timeline unless the user asked to move downstream content.
4. Use `clip_frame` to choose fit or fill for each shot; this changes framing, it does not fetch an image. Check the actual crop with `preview_frame`. Adjust manually with `clip_transform` if a face, object or on-screen text is cut off. When fill still hides important information, prefer fit and disclose the resulting margins. Place requested callouts with `title_add`; use `captions_add` only if you have real caption text and timing on the edited timeline. Do not add music, graphics or transitions unless requested.
5. Read back with `timeline_read` and inspect `preview_frame` images at the opening, important cuts and close. If visual proof is blocked, report the limitation and ask the user to inspect; stills cannot prove pacing or audio. Save the agreed editable project with `project_save` when the user wants it persisted. Do not render an MP4 as part of this skill.

If only a URL is available, ask for a legitimately downloaded local file. If timestamps, transcript, or beat markers are not available, ask for picks/timecodes or use whole shots where their purpose is known. Do not claim automatic transcribing, stock search, subject tracking, beat alignment or motion/audio review.
