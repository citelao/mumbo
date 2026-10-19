# TODO

Work tracker.

## State

- [ ] UI needs to work
- [ ] State tracking for users
- [ ] Managing user state (comments, volume adjustments, etc)
- [ ] Remove `Speaker`s on `UserRemove` or after inactivity; today they live for the whole connection

## Connection

- [ ] Replace the `CANCELLATION_TOKEN`/`CONNECTION` statics with Tauri managed state; a second Connect overwrites them while the old connection is still running, mixing two servers' users
- [ ] A webview reload leaves the old connection running in Rust with nobody listening
- [ ] `apply(…).unwrap()` and `channel.send(…).unwrap()` in the reader's `inspect` panic (and abort release builds) on a bad `UserState` or a closed window; log instead
- [ ] Unknown TCP/UDP message types end the connection; decode them to an `Unknown` variant instead

## Audio

- [ ] Input!
- [ ] Output & input device selection
- [ ] Device configuration (e.g. mono/stereo, bitrate resampling)
- [ ] Per-speaker auto-gain before summing, so one loud mic doesn't dominate
- [ ] Skipping empty NetEq buffers also skips concealment when a speaker briefly runs dry mid-sentence; track talk spurts (`is_terminator`) instead
- [ ] Move mixing (and the `cpal` stream) off the network task to an audio thread, so slow message handling can't starve playback
- [ ] Use Mumble's `frame_number` for NetEq sequence/timestamps before moving voice to UDP
