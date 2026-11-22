# Effects, filters and transitions

Look up catalogued packages and choose between a clip effect, a layer or a transition.

Call `effects_list` first. Optional `kind` is `effect`, `filter`, `audio`,
`transition` or `generator`. Use the returned id, parameter keys, defaults,
units and ranges. Display names are not ids.

| What to change | How |
| --- | --- |
| One clip's picture | Use `clip_update` with `path`, `clipId` and `patch.videoEffects`, an ordered list of `{id, params}` entries for picture effects and filters. Read the existing chain first and preserve entries you are keeping: the supplied list replaces the entire chain. |
| One clip's sound | Use `clip_update` with `patch.filters`, the separate audio chain, with ids from the audio catalogue. This list also replaces the entire chain. |
| Everything beneath a layer | Use `effect_layer_add` with `path`, `effectId`, `name`, `start` and optional `duration` (default 5 s). Omit `trackId` to place above the affected span; a named track must be above the intended picture. Set layer strength with `clip_update` and `patch.opacity` (0–1). |
| Color adjustments across a span | Confirm `concat.adjust` in the filter catalogue, then use it as `effectId` with `effect_layer_add`. Set its parameters on the layer's `videoEffects` chain using the catalogue's keys and ranges. |
| Audio mix across a span | Use `effect_layer_add` with `sound: true` and an audio `effectId`. This creates a `soundLayer`; use its `filters` chain for audio packages, `opacity` for wet/dry and `fadeIn`/`fadeOut` in seconds for ramps. It treats the audible mix during the span. |
| Incoming cut on a clip | Use `clip_update` with `patch.transitionIn: {id, duration}` on the incoming clip. Use a transition id and duration in seconds; `null` removes it. |
| Transition as its own timeline clip | Call `edit_apply` with `path` and `command` containing `op: "addTransitionClip"`, `transitionId`, `name`, `start` and optional `duration` and `trackId`. Start and duration are timeline seconds; duration defaults to 1 and is floored at 0.1. Omit `trackId` to place above the occupied span. Straddle the intended cut; over a single shot it plays over that shot. |

Before replacing a chain, use `project_get` if the summary omits settings
you need to preserve, such as disabled entries or parameter keyframes.
Read back the affected span with `timeline_read`. Use `preview_frame` before,
within and after the effect where useful. For a sound layer or a transition
whose motion matters, report whether actual playback was checked.

## What you cannot do

You cannot invoke uncatalogued effects or choose an editor preset card by its
display name. An audio effect does not generate speech.
Color and sharpness adjustments are not a dedicated image enhancement workflow.
Tell the user when the requested operation is unavailable; see the index.

Voice isolation is a clip mix, separate from the audio effect chain. There
is no dedicated MCP tool; use `edit_apply` with `setClipVoice` and `speech`
or `vocals` (see the index). Setting the mix does not verify model availability,
rendered separation, or how it sounds. Ask the user to check playback in the
app when separation matters.
