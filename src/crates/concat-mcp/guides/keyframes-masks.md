# Keyframes and masks

Animate clip properties, picture effects or shape masks using clip-relative key positions.

Read the target clip and its start and duration with `timeline_read`. All key
tools use `path`, `clipId`, normalized `at` (0 at clip start, 1 at clip end)
and a numeric `value` in the property's own units. Convert an absolute timeline
time to `(time - clip.start) / clip.duration` and keep it within 0–1. Optional
`ease` is `[x1, y1, x2, y2]` like a CSS cubic Bézier; omit for linear.

- `keyframe_set` targets `property`: `scale` (0.05–8), `offsetX`/`offsetY`
  (fractions of frame width/height, positive right/down), `rotation` (degrees),
  `opacity` (0–1) or `volume` (nonnegative gain; 1 is unchanged). Use
  `keyframe_clear` with the property and `at` to remove one key.
- `effect_key_set` targets a zero-based `entry` in the clip's `videoEffects`
  chain and a parameter `key`. Look up that package's key, units and value
  range with `effects_list`. Recheck the entry if the chain has been reordered.
- `clip_masks` replaces the `masks` array, top first. Preserve existing masks
  you want to keep; an empty array removes all. Give each mask you intend to
  animate an `id`. Use `source: {kind: "shape", shape, width, height, roundness}`.
  Valid shapes are `linear`, `mirror`, `circle`, `rectangle`, `heart`, `star`.
  Join to masks above with `mode`: `add`, `subtract`, `intersect` or `none`.
  `x`/`y` are fractions of the picture dimensions (positive right/down);
  `width`, `height` and `expand` are fractions of the picture's shorter side.
  `rotation` is degrees, `scale` is a multiplier, `feather`, `roundness` and
  `opacity` are 0–1, and `invert: true` keeps the outside.
- `mask_key_set` targets `maskId` and `number`: `x`, `y`, `rotation`, `scale`,
  `feather`, `expand`, `opacity`, `width`, `height` or `roundness`. Use the
  mask's own units. Setting a key near an existing time replaces that key.

Set both ends of an intended move and sample the middle with `preview_frame`.
Describe which positions were inspected; stills cannot prove smooth playback.

## What you cannot do

These tools do not track a subject automatically or animate arbitrary
properties. Use manually chosen positions or ask the user for reference times.

For singing separation and clip voice mixes, see ai-tools. `audio_separate_vocals`
creates the cache before `edit_apply` with `setClipVoice` sets the mix.
