//! Windows' "Universal Mute" (the mute key on newer laptops, and `Win+Alt+K`).
//!
//! Create a `UniversalMuter` to start the call, manage it during the call, and
//! then drop it to end the call.

#[cfg(not(windows))]
pub use other_impl::UniversalMuter;
#[cfg(windows)]
pub use windows_impl::UniversalMuter;

#[cfg(windows)]
mod windows_impl {
    use windows::{
        ApplicationModel::Calls::{
            MuteChangeEventArgs, VoipCallCoordinator, VoipPhoneCall, VoipPhoneCallMedia,
        },
        Foundation::TypedEventHandler,
        core::HSTRING,
    };

    pub type Error = windows::core::Error;

    pub struct UniversalMuter {
        coordinator: VoipCallCoordinator,
        call: VoipPhoneCall,
        mute_handler_token: i64,

        // What the button currently shows, so we only tell Windows about changes.
        shown_muted: bool,
    }

    impl UniversalMuter {
        /// Starts a call, with the button showing unmuted. `on_request` is
        /// called, from a Windows thread, each time the user presses the button
        /// (`true` = mute).
        pub fn new(on_request: impl Fn(bool) + Send + 'static) -> Result<Self, Error> {
            let coordinator = VoipCallCoordinator::GetDefault()?;

            let mute_handler_token = coordinator.MuteStateChanged(&TypedEventHandler::new(
                move |_, args: windows::core::Ref<MuteChangeEventArgs>| {
                    // Call the callback; when muting is confirmed, the app should call `confirm`.
                    on_request(args.ok()?.Muted()?);
                    Ok(())
                },
            ))?;

            let call = coordinator.RequestNewOutgoingCall(
                &HSTRING::from("mumbo"),
                // Shown until we learn which channel we're in.
                &HSTRING::from("Connecting..."),
                &HSTRING::from("Mumbo"),
                VoipPhoneCallMedia::Audio,
            )?;
            coordinator.NotifyUnmuted()?;
            call.NotifyCallActive()?;

            Ok(Self {
                coordinator,
                call,
                mute_handler_token,
                shown_muted: false,
            })
        }

        /// Shows the button as muted/unmuted. Call this when server mute state
        /// changes.
        pub fn confirm(&mut self, muted: bool) -> Result<(), Error> {
            if self.shown_muted == muted {
                return Ok(());
            }
            if muted {
                self.coordinator.NotifyMuted()?;
            } else {
                self.coordinator.NotifyUnmuted()?;
            }
            self.shown_muted = muted;
            Ok(())
        }

        /// Renames the call, which Windows shows as who you're talking to.
        pub fn set_contact_name(&self, name: &str) -> Result<(), Error> {
            self.call.SetContactName(&HSTRING::from(name))
        }
    }

    impl Drop for UniversalMuter {
        fn drop(&mut self) {
            // Nothing useful to do if Windows refuses; the process is likely going away.
            let _ = self
                .coordinator
                .RemoveMuteStateChanged(self.mute_handler_token);
            let _ = self.call.NotifyCallEnded();
        }
    }
}

#[cfg(not(windows))]
mod other_impl {
    use std::convert::Infallible;

    pub type Error = Infallible;

    pub struct UniversalMuter;

    impl UniversalMuter {
        pub fn new(_on_request: impl Fn(bool) + Send + 'static) -> Result<Self, Error> {
            Ok(Self)
        }

        pub fn confirm(&mut self, _muted: bool) -> Result<(), Error> {
            Ok(())
        }

        pub fn set_contact_name(&self, _name: &str) -> Result<(), Error> {
            Ok(())
        }
    }
}
