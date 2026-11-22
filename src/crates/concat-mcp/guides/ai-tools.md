# Local AI tools

Transcribe media, create narration, separate singing and enhance pictures with models installed on this machine.

1. Call `ai_status` with `{}`. Check the needed model and job activity in
   `transcription`, `speechGeneration`, `vocalSeparation` or `videoEnhancement`.
   A capability can be available even when its model is missing. Use
   `timeline_read` to confirm the open project's `path` and get actual IDs.
2. Start the appropriate job below. Keep the returned `job` and poll
   `export_status` with `{"job":"<id>"}`. `stage` and `fraction` show progress;
   use `export_cancel` with the same `job` to request a stop.
3. Use the result only after `state` is `done`. If it is `failed` or
   `cancelled`, read `error` and report what happened. A started job or an
   `output` path alone does not prove that a result is ready.

If the call returns `modelMissing`, tell the user which model is needed and
its reported size. Ask them to install it using the window controls below,
then check `ai_status` again. These tools run locally and do not download
models on the Agent's behalf.

## Transcribe and add captions

- Call `media_transcribe` with `path` and exactly one of `mediaId` (the source
  file's default audio stream) or `clipId` (the clip's selected stream and
  source range). Optional `language` is a Whisper language code such as `en`
  or `zh`; omit it for detection. Optional `modelId` selects a model from
  `ai_status`. Find missing models in Settings > Transcriber.
- After `done`, call `transcript_query` with `path` and the same target ID.
  Optional `start` and `end` filter by seconds from the source file's start;
  `keyword` matches segment text without regard to case. The result has
  `segments` and each segment's `words`, with `text`, `start` and `end`.
- Group timed words into readable captions. Convert the word boundaries to
  timeline seconds before calling `captions_add` with `path` and
  `captions: [{"start": ..., "end": ..., "text": ...}]`. For a forward clip
  at constant speed, subtract the clip's source in-point from each source
  boundary, divide by the speed and add the clip's timeline start. Keep
  captions inside the visible clip. Check the mapping separately for speed
  ramps, reversed or repeated sections. Use actual transcript text, and review
  uncertain words with the user. Check legibility with `preview_frame`.

## Generate narration and place it

- Call `speech_voices` with `{}`. Use an entry marked `available`; its `id`
  is the `voiceId` for `speech_generate`. Only stock voices are listed. If
  the chosen voice needs a model, install it in Settings > Speech.
- Call `speech_generate` with `path`, `text`, `voiceId` and optional `speed`
  (0.5–2; default 1). The job imports a WAV into the media bin. After `done`,
  read `mediaId` and `duration` from `export_status`.
- Call `clip_add` with `path`, that `mediaId`, the chosen `trackId` from
  `timeline_read`, and the desired timeline `start`. Read back its placement
  and ask for playback if pronunciation or sync matters.

## Separate singing, then set the clip mix

- Call `audio_separate_vocals` with `path` and exactly one of `clipId` (use
  the clip's selected audio stream) or `mediaId` (use the default stream).
  The job processes the full chosen stream and returns a cache folder. It
  does not change the mix. If Spleeter is missing, the user can select
  Voice Isolation > Song (人声分离 → 歌曲) in the clip inspector to install
  the two models (39,362,041 bytes total).
- Wait for `done`, then call `edit_apply` with the same project's `path`
  and the target timeline clip's ID in the command, for example:

```json
{"path":"/absolute/path/to/project","command":{"op":"setClipVoice","clipId":"c11","voice":{"kind":"vocals","voice":1,"background":0}}}
```

Both gains are 0–2 (0 mutes, 1 leaves level unchanged). For dialogue, the
`speech` mix is a separate `setClipVoice` option; the singing separation job
is for `vocals`. Read back the mix with `project_get`; confirm the audible
result by playing the affected interval in the app.

## Enhance a clip's picture

- Call `clip_enhance` with `path`, a video or image `clipId`, and optional
  `scale` (1–4, capped further by source size, or omit for Auto). If
  `realesr-general-x4v3` is missing (4,866,417 bytes), ask the user to turn on
  Enhance in the clip inspector and click Preview to download it.
- Enhancement runs on each frame, so longer video spans can take substantial
  time and disk space. Start with a short clip when judging whether it helps.
  The job creates an enhanced PNG or MP4 in the project cache and replaces
  the clip's source only on success; the original media stays intact.
  Poll until `done`, then check the clip with `timeline_read` and sample frames
  with `preview_frame`. Save the edit with `project_save` when requested.
