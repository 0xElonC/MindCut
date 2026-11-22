# Titles and captions

Add titles, style selected characters and place captions from supplied text and timing.

- Use `title_add` with `path`, timeline `start` in seconds and `style.content`.
  `duration` is in seconds (default 4). For an overlay, set `above: true` and
  omit `trackId`, or choose a known track above the picture. `offsetY` is a
  fraction of frame height from center (positive down, clamped to ±3).
  Set `style.fontFamily`, `fontSize` (fraction of frame height, default 0.09),
  `fontWeight` (100–900) and `color` (e.g. `#ffffff`) as needed.
- Use `title_style_words` with `path`, the title's `clipId`, `start`, `end`
  and `patch`. Count Unicode scalar values from zero, not bytes or grapheme
  clusters (a combining mark counts separately); `end` is exclusive.
  Overrides can include `color`, `fontWeight` (100–900), `italic`, `underline`,
  `strikethrough`, `fontScale` (0.1–10 times title size) or `strokeColor`.
  Omitted fields keep their previous value. `title_clear_word_styles` removes
  overrides on a supplied range, or the whole title if either endpoint is absent.
- Use `captions_add` with `path` and a nonempty `captions` array of
  `{start, end, text}`. Use edited timeline seconds, `start` ≥ 0 and `end` >
  `start`. Each entry becomes one title, all in one undo step. Optional `style`
  is shared; `content` is replaced with each entry's text. Default style is
  Hanken Grotesk, weight 600, size 0.04. `offsetY` ranges from −3 to 3, with
  0.35 near the bottom by default, 0 centered and −0.35 near the top.

Read ids and `textRuns` with `timeline_read`. Check legibility over the actual
footage with `preview_frame`, including around caption boundaries.

## From source audio to captions

Use `media_transcribe`, wait for the job, then read word timings with
`transcript_query`. Convert source seconds to timeline seconds before calling
`captions_add`. For new narration, use `speech_generate` and place the result
with `clip_add`. See ai-tools for model setup and timing examples. If real
words or timings are missing, ask the user; never invent them.

## What you cannot do

Title preset cards cannot be invoked by name. Ask for the look and set font,
size, color and weight explicitly.
