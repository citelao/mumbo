use std::collections::BTreeMap;
use std::io::{self, Error};

use serde::Serialize;

// See raw::UserState for the fields that correspond to this struct.
#[derive(Default, Clone, Debug, Serialize)]
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

impl User {
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Applies a protobuf delta: `None` means the field didn't change.
fn merge<T>(field: &mut T, update: Option<T>) {
    if let Some(value) = update {
        *field = value;
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ConnectionStatus {
    #[default]
    Connecting,

    Connected,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub enum MumbleEvent {
    // TODO: more granular events are probably useful, but let's start with this.
    UsersChanged,
    ChatMessage,
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct MumbleState {
    // Sorted by session, so snapshots (UI, fixtures) have a stable order.
    users: BTreeMap<u32, User>,

    connection_status: ConnectionStatus,

    // TODO: senders
    chat_messages: Vec<String>,
}

impl MumbleState {
    pub fn connection_status(&self) -> &ConnectionStatus {
        &self.connection_status
    }

    pub fn users(&self) -> Vec<User> {
        self.users.values().cloned().collect()
    }

    pub fn chat_messages(&self) -> &Vec<String> {
        &self.chat_messages
    }

    pub fn apply(&mut self, message: crate::ServerToClientMessage) -> Result<Option<MumbleEvent>, Error> {
        match message {
            crate::ServerToClientMessage::Reject(m) => {
                log::error!("Server rejected connection: {} - {}", m.r#type().as_str_name(), m.reason());
                self.connection_status = ConnectionStatus::Failed;
                return Err(Error::new(io::ErrorKind::PermissionDenied, format!("Server rejected connection: {} - {}", m.r#type().as_str_name(), m.reason())));
            }

            // Signals the end of the initial server handshake.
            crate::ServerToClientMessage::ServerSync(m) => {
                self.connection_status = ConnectionStatus::Connected;
                log::info!("Successfully connected to the server");

                // May include a welcome message, which we can add to the chat messages.
                if let Some(welcome_text) = m.welcome_text {
                    self.chat_messages.push(welcome_text);
                    // TODO: support multiple events for this
                    return Ok(Some(MumbleEvent::ChatMessage));
                }
            },

            // TODO: unsure if this is a chat message.
            // crate::ServerToClientMessage::ServerConfig(_) => {
            //     if let Some(welcome_text) = m.welcome_text {
            //         self.chat_messages.push(welcome_text);
            //         // TODO: support multiple events for this
            //         return Ok(Some(MumbleEvent::ChatMessage));
            //     }
            // }

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
                return Ok(Some(MumbleEvent::UsersChanged));
            },
            crate::ServerToClientMessage::UserRemove(m) => {
                let removed = self.users.remove(&m.session);
                if let Some(removed) = removed {
                    log::info!("Removed user: {} (session: {}) for reason: {:?} (ban: {:?})", removed.name, removed.session, m.reason, m.ban);
                    return Ok(Some(MumbleEvent::UsersChanged));
                } else {
                    return Err(Error::new(io::ErrorKind::InvalidData, "User not found"));
                }
            }

            // Chat messages
            crate::ServerToClientMessage::TextMessage(m) => {
                self.chat_messages.push(m.message);
                return Ok(Some(MumbleEvent::ChatMessage));
            }

            _ => {
                log::warn!("Unhandled message: {:?}", message);
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_state(session: u32, name: &str) -> crate::ServerToClientMessage {
        crate::ServerToClientMessage::UserState(crate::raw::UserState {
            session: Some(session),
            name: Some(name.to_string()),
            ..Default::default()
        })
    }

    #[test]
    fn text_messages_are_kept_in_order_and_reported() {
        let mut state = MumbleState::default();

        for text in ["hi", "how's it going"] {
            let event = state
                .apply(crate::ServerToClientMessage::TextMessage(crate::raw::TextMessage {
                    message: text.to_string(),
                    ..Default::default()
                }))
                .unwrap();
            assert!(matches!(event, Some(MumbleEvent::ChatMessage)), "{event:?}");
        }

        assert_eq!(state.chat_messages(), &["hi", "how's it going"]);
    }

    #[test]
    fn server_sync_connects_and_shows_the_welcome_text_in_chat() {
        let mut state = MumbleState::default();

        let event = state
            .apply(crate::ServerToClientMessage::ServerSync(crate::raw::ServerSync {
                welcome_text: Some("Welcome!".to_string()),
                ..Default::default()
            }))
            .unwrap();

        assert_eq!(state.connection_status(), &ConnectionStatus::Connected);
        assert_eq!(state.chat_messages(), &["Welcome!"]);
        assert!(matches!(event, Some(MumbleEvent::ChatMessage)), "{event:?}");
    }

    #[test]
    fn server_sync_without_welcome_text_connects_quietly() {
        let mut state = MumbleState::default();

        let event = state.apply(crate::ServerToClientMessage::ServerSync(Default::default())).unwrap();

        assert_eq!(state.connection_status(), &ConnectionStatus::Connected);
        assert!(state.chat_messages().is_empty());
        assert!(event.is_none(), "{event:?}");
    }

    #[test]
    fn user_joins_updates_and_leaves_are_reported() {
        let mut state = MumbleState::default();

        let joined = state.apply(user_state(1, "Alice")).unwrap();
        let renamed = state.apply(user_state(1, "Alice Cooper")).unwrap();
        let left = state
            .apply(crate::ServerToClientMessage::UserRemove(crate::raw::UserRemove {
                session: 1,
                ..Default::default()
            }))
            .unwrap();

        for event in [joined, renamed, left] {
            assert!(matches!(event, Some(MumbleEvent::UsersChanged)), "{event:?}");
        }
        assert!(state.users().is_empty());
    }

    #[test]
    fn rejection_fails_with_the_server_reason() {
        let mut state = MumbleState::default();

        let error = state
            .apply(crate::ServerToClientMessage::Reject(crate::raw::Reject {
                r#type: Some(crate::raw::reject::RejectType::WrongServerPw as i32),
                reason: Some("Invalid server password".to_string()),
            }))
            .unwrap_err();

        assert!(error.to_string().contains("Invalid server password"), "{error}");
        assert_eq!(state.connection_status(), &ConnectionStatus::Failed);
    }

    // src/mumbleState.test.ts parses the same fixture with the zod `User`
    // schema, so drift on either side fails a test.
    // Snapshots go to the UI and fixtures, so their order must not depend on
    // hashing.
    #[test]
    fn users_are_ordered_by_session() {
        let mut state = MumbleState::default();
        for session in (1..=20).rev() {
            state
                .apply(crate::ServerToClientMessage::UserState(crate::raw::UserState {
                    session: Some(session),
                    ..Default::default()
                }))
                .unwrap();
        }

        let sessions: Vec<u32> = state.users().iter().map(|user| user.session).collect();
        assert_eq!(sessions, (1..=20).collect::<Vec<_>>());
    }

    #[test]
    fn users_match_shared_fixture() {
        let mut state = MumbleState::default();
        state
            .apply(crate::ServerToClientMessage::UserState(crate::raw::UserState {
                session: Some(7),
                name: Some("Alice".to_string()),
                channel_id: Some(3),
                self_mute: Some(true),
                ..Default::default()
            }))
            .unwrap();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/users.json")).unwrap();

        assert_eq!(serde_json::to_value(state.users()).unwrap(), fixture);
    }

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
        })).unwrap();
        
        // Check that the user was added correctly
        let user = state.users.get(&1).unwrap();
        assert_eq!(user.session, 1);
        assert_eq!(user.name, "Alice");
        assert_eq!(user.channel_id, 1);
        assert!(!user.mute);
        assert!(!user.deaf);
        assert!(!user.suppress);
        assert!(!user.self_mute);
        assert!(!user.self_deaf);
        assert!(!user.priority_speaker);
        assert!(!user.recording);

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
        })).unwrap();

        let user = state.users.get(&1).unwrap();
        assert_eq!(user.session, 1);
        assert_eq!(user.name, "Alice Cooper");
        assert_eq!(user.channel_id, 2);
        assert!(user.mute);
        assert!(!user.deaf);
        assert!(user.suppress);
        assert!(user.self_mute);
        assert!(!user.self_deaf);
        assert!(!user.priority_speaker);
        assert!(!user.recording);
    }

    #[test]
    fn test_add_remove_user() {
        let mut state = MumbleState::default();

        // Create a user
        state.apply(crate::ServerToClientMessage::UserState({
            crate::raw::UserState{
                session: Some(1),
                name: Some("Alice".to_string()),
                ..Default::default()
            }
        })).unwrap();
        let user = state.users.get(&1).unwrap();
        assert_eq!(user.session, 1);
        assert_eq!(user.name, "Alice");

        // Remove the user
        state.apply(crate::ServerToClientMessage::UserRemove({
            crate::raw::UserRemove {
                session: 1,
                ..Default::default()
            }
        })).unwrap();
        assert!(!state.users.contains_key(&1));
    }
}