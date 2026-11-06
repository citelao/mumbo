# TODO

Work tracker.

## State

- [x] UI needs to work
- [x] State tracking for users
- [ ] Managing user state (comments, volume adjustments, etc)
- [ ] Remove `Speaker`s on `UserRemove` or after inactivity; today they live for the whole connection
- [ ] Gracefully disconnect server rather than panicking:

    > [2026-10-20][01:52:30][DEBUG][mumble_protocol::client] Received server message: UserRemove(UserRemove { session: 24, actor: None, reason: Some("You connected to the server from another device"), ban: None })
    > [2026-10-20][01:52:30][INFO][mumble_protocol::state] Removed user: testo (session: 24) for reason: Some("You connected to the server from another device") (ban: None)
- [ ] Show state changes in chat log?

## Connection

- [ ] Verify SSL certs when requested.
- [ ] Replace the `CANCELLATION_TOKEN`/`CONNECTION` statics with Tauri managed state; a second Connect overwrites them while the old connection is still running, mixing two servers' users
- [ ] A webview reload leaves the old connection running in Rust with nobody listening
- [ ] `apply(…).unwrap()` and `channel.send(…).unwrap()` in the reader's `inspect` panic (and abort release builds) on a bad `UserState` or a closed window; log instead
- [ ] Unknown TCP/UDP message types end the connection; decode them to an `Unknown` variant instead
- [ ] Send OS version info?

## Audio

- [x] Input!
- [x] Output & input device selection
- [ ] Device selection during a call
- [x] Hook up audio selectors
- [ ] BUG: AEC should use full system audio, since that shouldn't be included.
- [ ] Device configuration (e.g. mono/stereo, bitrate resampling)
- [ ] Per-speaker auto-gain before summing, so one loud mic doesn't dominate
- [ ] Other speaker local volume adjustment
- [ ] Handle custom speaker gain adjustments
- [x] Mute/unmute self
- [ ] Skipping empty NetEq buffers also skips concealment when a speaker briefly runs dry mid-sentence; track talk spurts (`is_terminator`) instead
- [ ] Move mixing (and the `cpal` stream) off the network task to an audio thread, so slow message handling can't starve playback
- [ ] Use Mumble's `frame_number` for NetEq sequence/timestamps before moving voice to UDP
- [ ] BUG: tapping the microphone on a mac seems to trigger audio, even though no speech is present.
- [ ] Streaming a pure tone through NetEq in real time comes out ~2% sharp (450 Hz for 440 Hz): accelerate removes ~23% of the audio to hold a 20 ms target delay. Check speech for artifacts; consider a higher `min_delay_ms` (see the e2e test in lib.rs)

## Users
- [ ] Profile pics
- [ ] Comments
- [ ] Signed accounts
- [ ] Local nicknames (only seems to work with certificated users)
- [ ] Information screen with stats (& cert?)?

## Channels

- [x] Channels visualization
- [x] Nested channel visualizaiton
- [x] Switching channels
- [ ] Creating channels (when you have permission)
- [ ] Editing channels (when you have permission)
- [ ] "Listen in" to a channel
- [ ] Actual keyboard navigation of channels; accessible list
- [ ] Enter password-protected channels

## Messages

- [x] Distinguish DMs vs global messages
- [ ] Distinguish channel vs global messages
- [x] Send DMs
- [ ] Send actual DMs, with nice UI
- [ ] Support mrkdwn & wysiwyg chat messages
- [ ] Notifications?
- [ ] Notification settings
- [ ] TTS for messages?

## Home page

- [x] Refresh the members list every few sec.
- [x] Show latency stats (like iOS!)
- [ ] Allow saving favorites?
- [ ] Show common public servers?
- [ ] Save passwords for recent servers in the OS keychain (`keyring` crate), not in the plain-JSON store
- [ ] Include a help link
- [ ] BUG: servers that are offline still appear in recents (should show error)
- [ ] Join URLs (including to channels)