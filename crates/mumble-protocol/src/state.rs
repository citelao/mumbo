use std::{collections::HashMap};
use std::io::{self, Error};

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

/// Applies a protobuf delta: `None` means the field didn't change.
fn merge<T>(field: &mut T, update: Option<T>) {
    if let Some(value) = update {
        *field = value;
    }
}

#[derive(Default)]
pub struct MumbleState {
    users: HashMap<u32, User>
}

impl MumbleState {
    pub fn apply(&mut self, message: crate::ServerToClientMessage) -> Result<(), Error> {
        match message {
            crate::ServerToClientMessage::UserState(m) => {
                let Some(session) = m.session else {
                    return Err(Error::new(io::ErrorKind::InvalidData, "Missing session"));
                };

                let user = self.users
                    .entry(session)
                    .or_insert_with(|| User {
                        session,
                        ..Default::default()
                    });

                // Set any fields we know about!
                merge(&mut user.name, m.name);
                merge(&mut user.channel_id, m.channel_id);
                merge(&mut user.mute, m.mute);
                merge(&mut user.deaf, m.deaf);
                merge(&mut user.suppress, m.suppress);
                merge(&mut user.self_mute, m.self_mute);
                merge(&mut user.self_deaf, m.self_deaf);
                merge(&mut user.priority_speaker, m.priority_speaker);
                merge(&mut user.recording, m.recording);
            },
            _ => {
                log::warn!("Unhandled message: {:?}", message);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply() {
        let mut state = MumbleState::default();

        // Create a user
        state.apply(crate::ServerToClientMessage::UserState({
            crate::raw::UserState{
                session: Some(1),
                name: Some("Alice".to_string()),
                channel_id: Some(1),
                mute: Some(false),
                deaf: Some(false),
                suppress: Some(false),
                self_mute: Some(false),
                self_deaf: Some(false),
                priority_speaker: Some(false),
                recording: Some(false),
                ..Default::default()
            }
        }))?;
        
        // Check that the user was added correctly
        let user = state.users.get(&1).unwrap();
        assert_eq!(user.session, 1);
        assert_eq!(user.name, "Alice");
        assert_eq!(user.channel_id, 1);
        assert_eq!(user.mute, false);
        assert_eq!(user.deaf, false);
        assert_eq!(user.suppress, false);
        assert_eq!(user.self_mute, false);
        assert_eq!(user.self_deaf, false);
        assert_eq!(user.priority_speaker, false);
        assert_eq!(user.recording, false);

        // Change name & some state
        state.apply(crate::ServerToClientMessage::UserState({
            crate::raw::UserState {
                session: Some(1),
                name: Some("Alice Cooper".to_string()),
                channel_id: Some(2),
                mute: Some(true),
                suppress: Some(true),
                self_mute: Some(true),
                ..Default::default()
            }
        }))?;

        let user = state.users.get(&1).unwrap();
        assert_eq!(user.session, 1);
        assert_eq!(user.name, "Alice Cooper");
        assert_eq!(user.channel_id, 2);
        assert_eq!(user.mute, true);
        assert_eq!(user.deaf, false);
        assert_eq!(user.suppress, true);
        assert_eq!(user.self_mute, true);
        assert_eq!(user.self_deaf, false);
        assert_eq!(user.priority_speaker, false);
        assert_eq!(user.recording, false);
    }
}