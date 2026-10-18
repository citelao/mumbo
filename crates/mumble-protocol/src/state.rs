use std::collections::HashMap;

// See raw::UserState for the fields that correspond to this struct.
#[derive(Default)]
pub struct User {
    // Unique user ID
    session: u32,

    // Display name of the user
    name: String,

    // Channel ID the user is in
    channel_id: u32,

    // Whether the user is muted by the server
    mute: bool,

    // Whether the user is deafened by the server
    deaf: bool,

    // Whether the user is suppressed by the server
    suppress: bool,

    // Whether user is self-muted
    self_mute: bool,

    // Whether user is self-deafened
    self_deaf: bool,

    // Whether the user is a priority speaker
    priority_speaker: bool,

    // Whether the user is recording
    recording: bool,
    
    // actor?
    // user_id?
    // texture?
    // comment?
    // hash?
    // comment_hash?
    // texture_hash?
    // temporary_access_tokens?
    // listening_channel_add?
    // listening_channel_remove?
    // listening_volume_adjustment?

    // NOT plugin_context
    // NOT plugin_identity

}

#[derive(Default)]
pub struct MumbleState {
    users: HashMap<u32, User>
}

impl MumbleState {
    pub fn apply(&mut self, message: crate::ServerToClientMessage) {
        match message {
            crate::ServerToClientMessage::UserState(m) => {
                let session = m.session.unwrap();

                let user = self.users
                    .entry(session)
                    .or_insert_with(|| User {
                        session,
                        ..Default::default()
                    });

                // Set any fields we know about!
                if let Some(name) = m.name {
                    user.name = name;
                }
                if let Some(channel_id) = m.channel_id {
                    user.channel_id = channel_id;
                }
                if let Some(mute) = m.mute {
                    user.mute = mute;
                }
                if let Some(deaf) = m.deaf {
                    user.deaf = deaf;
                }
                if let Some(suppress) = m.suppress {
                    user.suppress = suppress;
                }
                if let Some(self_mute) = m.self_mute {
                    user.self_mute = self_mute;
                }
                if let Some(self_deaf) = m.self_deaf {
                    user.self_deaf = self_deaf;
                }
                if let Some(priority_speaker) = m.priority_speaker {
                    user.priority_speaker = priority_speaker;
                }
                if let Some(recording) = m.recording {
                    user.recording = recording;
                }
            },
            _ => {
                log::warn!("Unhandled message: {:?}", message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply() {
        let mut state = MumbleState::default();
        // Example test, replace with actual message
        // state.apply(crate::ServerToClientMessage::Version(...));
    }
}