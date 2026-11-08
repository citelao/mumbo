<!--
2-sentence summary: what is current behavior, what is fix?

> Today, audio does not play on macOS because we use the wrong API. This
> change fixes macOS audio playback by consuming the new `VoiceAudio` API.
-->
Today [X]. This change [Y].

## What changed?

<!--
A bulleted list of what you changed, plus any additional notes.

> - Added audio crate.
> - Consumed audio crate in `macOS/` dir.
> - Added unit tests.
-->

- [...]

## How tested?

<!--
How did you verify this change? What manual tests did you run? Any issues?

> Deployed on macOS locally.
>
> - [x] Audio plays back.
> - [x] After joining & rejoining, audio still plays.
> - [ ] ~~Video plays back~~ video has been postponed to #123.
-->