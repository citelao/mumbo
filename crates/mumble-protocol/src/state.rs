use std::collections::BTreeMap;
use std::io::{self, Error};

use chrono::{DateTime, Utc};
use serde::Serialize;

// See raw::UserState for the fields that correspond to this struct.
#[derive(Default, Clone, Debug, Serialize)]
pub struct User {
    // Unique user ID
    pub session: u32,

    // Display name of the user
    pub name: String,

    // Channel ID the user is in
    pub channel_id: u32,

    // Whether the user is muted by the server
    pub mute: bool,

    // Whether the user is deafened by the server
    pub deaf: bool,

    // Whether the user is suppressed by the server
    pub suppress: bool,

    // Whether user is self-muted
    pub self_mute: bool,

    // Whether user is self-deafened
    pub self_deaf: bool,

    // Whether the user is a priority speaker
    pub priority_speaker: bool,

    // Whether the user is recording
    pub recording: bool,

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

    // Additional fields
    pub is_me: bool,
    pub is_speaking: bool,
}

#[derive(Default, Debug, Clone, Serialize)]
pub struct Channel {
    // Unique channel ID
    pub channel_id: u32,

    // Parent channel, if any
    pub parent: Option<u32>,

    // Display name of the channel
    pub name: String,

    // Description of the channel
    pub description: String,

    // Does this channel have a password?
    pub is_enter_restricted: bool,

    // ???
    pub can_enter: bool,

    // TODO: links, temporary? position, description_hash, max_users, can_enter
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTextMessage {
    // The time the message was sent
    pub timestamp: DateTime<Utc>,

    // The user who sent the message
    pub user_session: Option<u32>,

    // The content of the message; may be HTML.
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerTextMessage {
    // The time the server message was sent
    pub timestamp: DateTime<Utc>,

    // The content of the server message; may be HTML.
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum TextMessage {
    ChatTextMessage(ChatTextMessage),
    ServerTextMessage(ServerTextMessage),
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
    ChannelsChanged,
    ChatMessage,
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct MumbleState {
    // Sorted by session, so snapshots (UI, fixtures) have a stable order.
    pub users: BTreeMap<u32, User>,

    // Sorted by channel ID, so snapshots (UI, fixtures) have a stable order.
    pub channels: BTreeMap<u32, Channel>,

    connection_status: ConnectionStatus,

    // Session (ID) for the current user
    pub current_user_session: Option<u32>,

    pub chat_messages: Vec<TextMessage>,
}

impl MumbleState {
    pub fn connection_status(&self) -> &ConnectionStatus {
        &self.connection_status
    }

    pub fn users(&self) -> Vec<User> {
        self.users.values().cloned().collect()
    }

    pub fn channels(&self) -> Vec<Channel> {
        self.channels.values().cloned().collect()
    }

    pub fn current_user(&self) -> Option<&User> {
        let current_user_session = self.current_user_session?;
        self.users.get(&current_user_session)
    }

    pub fn chat_messages(&self) -> &Vec<TextMessage> {
        &self.chat_messages
    }

    /// Records whether `session` is speaking. Only reports actual changes,
    /// since voice packets arrive 50-100 times a second per speaker.
    pub(crate) fn set_speaking(&mut self, session: u32, is_speaking: bool) -> Option<MumbleEvent> {
        let user = self.users.get_mut(&session)?;
        if user.is_speaking == is_speaking {
            return None;
        }
        user.is_speaking = is_speaking;
        Some(MumbleEvent::UsersChanged)
    }

    pub fn apply(&mut self, message: crate::ServerToClientMessage) -> Result<Option<MumbleEvent>, Error> {
        match message {
            crate::ServerToClientMessage::Ping(_) => {
                // Ignore for now.
                // TODO: track round-trip time etc.
            }
            
            crate::ServerToClientMessage::Reject(m) => {
                log::error!("Server rejected connection: {} - {}", m.r#type().as_str_name(), m.reason());
                self.connection_status = ConnectionStatus::Failed;
                return Err(Error::new(io::ErrorKind::PermissionDenied, format!("Server rejected connection: {} - {}", m.r#type().as_str_name(), m.reason())));
            }

            // Signals the end of the initial server handshake.
            crate::ServerToClientMessage::ServerSync(m) => {
                // The server sends one ServerSync per connection; a second would
                // leave two users flagged as us.
                if self.current_user_session.is_some() {
                    return Err(Error::new(io::ErrorKind::InvalidData, "Duplicate ServerSync"));
                }

                self.current_user_session = m.session;
                if let Some(session) = self.current_user_session {
                    // Set me!
                    if let Some(user) = self.users.get_mut(&session) {
                        user.is_me = true;
                    }
                }

                self.connection_status = ConnectionStatus::Connected;
                log::info!("Successfully connected to the server");

                // May include a welcome message, which we can add to the chat messages.
                if let Some(welcome_text) = m.welcome_text {
                    self.chat_messages.push(TextMessage::ServerTextMessage(ServerTextMessage {
                        timestamp: chrono::Utc::now(),
                        content: welcome_text,
                    }));
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

                // Channels arrive before users in the handshake, so an unknown
                // channel is a protocol error. Checked before touching state.
                if let Some(channel_id) = m.channel_id {
                    if !self.channels.contains_key(&channel_id) {
                        return Err(Error::new(io::ErrorKind::InvalidData, format!("Unknown channel: {channel_id}")));
                    }
                }

                let current_user_session = self.current_user_session;
                let user = self.users
                    .entry(session)
                    .or_insert_with(|| User {
                        session,
                        // ServerSync can (in theory) precede our own UserState.
                        is_me: current_user_session == Some(session),
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
                log::info!("Updated user: {} (session: {})", user.name, user.session);
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
            crate::ServerToClientMessage::UdpTunnel(crate::ServerToClientUdpMessage::Audio(m)) => {
                return Ok(self.set_speaking(m.sender_session, !m.is_terminator));
            }

            // Chat messages
            crate::ServerToClientMessage::TextMessage(m) => {
                // There are a few fields that tell how the msg reached you, e.g. via DM or channel message.
                // TODO: expose them.

                self.chat_messages.push(TextMessage::ChatTextMessage(ChatTextMessage {
                    timestamp: chrono::Utc::now(),
                    content: m.message,
                    user_session: m.actor,
                }));
                return Ok(Some(MumbleEvent::ChatMessage));
            }

            // Channel state messages
            crate::ServerToClientMessage::ChannelState(m) => {
                let Some(channel_id) = m.channel_id else {
                    return Err(Error::new(io::ErrorKind::InvalidData, "Channel ID missing"));
                };

                let channel = self.channels
                    .entry(channel_id)
                    .or_insert_with(|| Channel {
                        channel_id: channel_id,
                        ..Default::default()
                    });

                merge(&mut channel.name, m.name);
                // The root channel never has a parent, so `None` only ever means "unchanged".
                if m.parent.is_some() {
                    channel.parent = m.parent;
                }
                merge(&mut channel.description, m.description);
                merge(&mut channel.is_enter_restricted, m.is_enter_restricted);
                merge(&mut channel.can_enter, m.can_enter);

                log::info!("Updated channel: {} (ID: {})", channel.name, channel.channel_id);
                return Ok(Some(MumbleEvent::ChannelsChanged));
            }
            crate::ServerToClientMessage::ChannelRemove(m) => {
                let removed = self.channels.remove(&m.channel_id);
                if let Some(removed) = removed {
                    log::info!("Removed channel: {} (ID: {})", removed.name, removed.channel_id);
                    return Ok(Some(MumbleEvent::ChannelsChanged));
                } else {
                    return Err(Error::new(io::ErrorKind::InvalidData, "Channel not found"));
                }
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

    /// A voice packet from `session`, as the server relays it.
    fn voice(session: u32, is_terminator: bool) -> crate::ServerToClientMessage {
        crate::ServerToClientMessage::UdpTunnel(crate::ServerToClientUdpMessage::Audio(crate::raw_udp::Audio {
            sender_session: session,
            opus_data: vec![1, 2, 3],
            is_terminator,
            ..Default::default()
        }))
    }

    /// Sessions of the users currently speaking.
    fn speaking(state: &MumbleState) -> Vec<u32> {
        state.users().iter().filter(|u| u.is_speaking).map(|u| u.session).collect()
    }

    /// Each chat line as (sender session, content); server messages have no sender.
    fn chat_log(state: &MumbleState) -> Vec<(Option<u32>, &str)> {
        state
            .chat_messages()
            .iter()
            .map(|m| match m {
                TextMessage::ChatTextMessage(m) => (m.user_session, m.content.as_str()),
                TextMessage::ServerTextMessage(m) => (None, m.content.as_str()),
            })
            .collect()
    }

    #[test]
    fn text_messages_are_kept_in_order_and_reported() {
        let mut state = MumbleState::default();

        // `actor` is the sender's session; the server fills it in for user messages.
        for (actor, text) in [(Some(1), "hi"), (Some(2), "how's it going"), (None, "who said that?")] {
            let event = state
                .apply(crate::ServerToClientMessage::TextMessage(crate::raw::TextMessage {
                    actor,
                    message: text.to_string(),
                    ..Default::default()
                }))
                .unwrap();
            assert!(matches!(event, Some(MumbleEvent::ChatMessage)), "{event:?}");
        }

        assert_eq!(
            chat_log(&state),
            [(Some(1), "hi"), (Some(2), "how's it going"), (None, "who said that?")]
        );
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
        assert!(
            matches!(&state.chat_messages()[..], [TextMessage::ServerTextMessage(m)] if m.content == "Welcome!"),
            "{:?}",
            state.chat_messages()
        );
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
        state.apply(channel_state(3, "Lobby", Some(0))).unwrap();
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
        state.apply(channel_state(1, "Lobby", Some(0))).unwrap();
        state.apply(channel_state(2, "Games", Some(0))).unwrap();

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

    #[test]
    fn new_users_are_not_speaking() {
        let mut state = MumbleState::default();

        state.apply(user_state(1, "Alice")).unwrap();

        assert_eq!(speaking(&state), Vec::<u32>::new());
    }

    #[test]
    fn voice_marks_the_sender_as_speaking() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();

        let event = state.apply(voice(1, false)).unwrap();

        assert_eq!(speaking(&state), [1]);
        assert!(matches!(event, Some(MumbleEvent::UsersChanged)), "{event:?}");
    }

    #[test]
    fn a_terminator_marks_the_sender_as_stopped() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();
        state.apply(voice(1, false)).unwrap();

        let event = state.apply(voice(1, true)).unwrap();

        assert_eq!(speaking(&state), Vec::<u32>::new());
        assert!(matches!(event, Some(MumbleEvent::UsersChanged)), "{event:?}");
    }

    #[test]
    fn speaking_again_after_a_terminator() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();

        state.apply(voice(1, false)).unwrap();
        state.apply(voice(1, true)).unwrap();
        state.apply(voice(1, false)).unwrap();

        assert_eq!(speaking(&state), [1]);
    }

    #[test]
    fn only_the_sender_is_marked_as_speaking() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();
        state.apply(user_state(2, "Bob")).unwrap();
        state.apply(user_state(3, "Carol")).unwrap();

        state.apply(voice(1, false)).unwrap();
        state.apply(voice(3, false)).unwrap();
        state.apply(voice(3, true)).unwrap();

        assert_eq!(speaking(&state), [1]);
    }

    #[test]
    fn user_state_updates_keep_speaking() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();
        state.apply(voice(1, false)).unwrap();

        // E.g. Alice mutes herself mid-sentence; her audio is still arriving.
        state
            .apply(crate::ServerToClientMessage::UserState(crate::raw::UserState {
                session: Some(1),
                self_mute: Some(true),
                ..Default::default()
            }))
            .unwrap();

        assert_eq!(speaking(&state), [1]);
    }

    #[test]
    fn speaking_users_who_leave_are_removed() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();
        state.apply(voice(1, false)).unwrap();

        state
            .apply(crate::ServerToClientMessage::UserRemove(crate::raw::UserRemove {
                session: 1,
                ..Default::default()
            }))
            .unwrap();
        state.apply(user_state(1, "Alice")).unwrap();

        // A later user reusing the session starts out quiet.
        assert_eq!(speaking(&state), Vec::<u32>::new());
    }

    #[test]
    fn voice_from_an_unknown_session_changes_nothing() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();

        let event = state.apply(voice(99, false)).unwrap();

        assert_eq!(state.users().len(), 1, "voice must not create users");
        assert_eq!(speaking(&state), Vec::<u32>::new());
        assert!(event.is_none(), "nothing changed, so nothing to report: {event:?}");
    }

    // Voice arrives 50-100 times a second per speaker; reporting each packet
    // would make the UI refetch users that often.
    #[test]
    fn continued_speech_is_reported_once() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();

        let events: Vec<_> = (0..5).map(|_| state.apply(voice(1, false)).unwrap()).collect();

        assert!(matches!(events[0], Some(MumbleEvent::UsersChanged)), "{events:?}");
        assert!(events[1..].iter().all(Option::is_none), "{events:?}");
    }

    fn channel_state(id: u32, name: &str, parent: Option<u32>) -> crate::ServerToClientMessage {
        crate::ServerToClientMessage::ChannelState(crate::raw::ChannelState {
            channel_id: Some(id),
            name: Some(name.to_string()),
            parent,
            ..Default::default()
        })
    }

    fn user_in_channel(session: u32, channel_id: u32) -> crate::ServerToClientMessage {
        crate::ServerToClientMessage::UserState(crate::raw::UserState {
            session: Some(session),
            channel_id: Some(channel_id),
            ..Default::default()
        })
    }

    fn server_sync(session: Option<u32>) -> crate::ServerToClientMessage {
        crate::ServerToClientMessage::ServerSync(crate::raw::ServerSync {
            session,
            ..Default::default()
        })
    }

    /// Sessions of the users flagged as the current user.
    fn me(state: &MumbleState) -> Vec<u32> {
        state.users().iter().filter(|u| u.is_me).map(|u| u.session).collect()
    }

    // src/invoke/mumbleState.test.ts parses the same fixture with the zod
    // `Channel` schema.
    #[test]
    fn channels_match_shared_fixture() {
        let mut state = MumbleState::default();
        state.apply(channel_state(0, "Root", None)).unwrap();
        state
            .apply(crate::ServerToClientMessage::ChannelState(crate::raw::ChannelState {
                channel_id: Some(3),
                name: Some("Lobby".to_string()),
                parent: Some(0),
                description: Some("Say hi".to_string()),
                ..Default::default()
            }))
            .unwrap();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/channels.json")).unwrap();

        assert_eq!(serde_json::to_value(state.channels()).unwrap(), fixture);
    }

    #[test]
    fn the_root_channel_has_no_parent() {
        let mut state = MumbleState::default();

        state.apply(channel_state(0, "Root", None)).unwrap();
        state.apply(channel_state(1, "Lobby", Some(0))).unwrap();

        assert_eq!(state.channels[&0].parent, None);
        assert_eq!(state.channels[&1].parent, Some(0));
    }

    #[test]
    fn channel_updates_are_reported() {
        let mut state = MumbleState::default();

        let created = state.apply(channel_state(1, "Lobby", Some(0))).unwrap();
        let renamed = state.apply(channel_state(1, "Hall", Some(0))).unwrap();

        for event in [created, renamed] {
            assert!(matches!(event, Some(MumbleEvent::ChannelsChanged)), "{event:?}");
        }
        assert_eq!(state.channels().len(), 1);
        assert_eq!(state.channels[&1].name, "Hall");
    }

    // The server only sends the fields that changed, so a rename carries no
    // `parent`; that must not move the channel to the root.
    #[test]
    fn renaming_a_channel_keeps_its_parent_and_description() {
        let mut state = MumbleState::default();
        state
            .apply(crate::ServerToClientMessage::ChannelState(crate::raw::ChannelState {
                channel_id: Some(1),
                name: Some("Lobby".to_string()),
                parent: Some(0),
                description: Some("Say hi".to_string()),
                ..Default::default()
            }))
            .unwrap();

        state
            .apply(crate::ServerToClientMessage::ChannelState(crate::raw::ChannelState {
                channel_id: Some(1),
                name: Some("Hall".to_string()),
                ..Default::default()
            }))
            .unwrap();

        let channel = &state.channels[&1];
        assert_eq!(channel.name, "Hall");
        assert_eq!(channel.parent, Some(0));
        assert_eq!(channel.description, "Say hi");
    }

    #[test]
    fn reparenting_a_channel_moves_it() {
        let mut state = MumbleState::default();
        state.apply(channel_state(0, "Root", None)).unwrap();
        state.apply(channel_state(1, "Lobby", Some(0))).unwrap();
        state.apply(channel_state(2, "Games", Some(0))).unwrap();

        state
            .apply(crate::ServerToClientMessage::ChannelState(crate::raw::ChannelState {
                channel_id: Some(1),
                parent: Some(2),
                ..Default::default()
            }))
            .unwrap();

        assert_eq!(state.channels[&1].parent, Some(2));
        assert_eq!(state.channels[&1].name, "Lobby");
    }

    #[test]
    fn channel_state_without_an_id_is_an_error() {
        let mut state = MumbleState::default();

        let error = state
            .apply(crate::ServerToClientMessage::ChannelState(Default::default()))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn removing_a_channel_is_reported() {
        let mut state = MumbleState::default();
        state.apply(channel_state(1, "Lobby", Some(0))).unwrap();

        let event = state
            .apply(crate::ServerToClientMessage::ChannelRemove(crate::raw::ChannelRemove { channel_id: 1 }))
            .unwrap();

        assert!(matches!(event, Some(MumbleEvent::ChannelsChanged)), "{event:?}");
        assert!(state.channels().is_empty());
    }

    #[test]
    fn removing_an_unknown_channel_is_an_error() {
        let mut state = MumbleState::default();

        let error = state
            .apply(crate::ServerToClientMessage::ChannelRemove(crate::raw::ChannelRemove { channel_id: 9 }))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    fn channel_remove(channel_id: u32) -> crate::ServerToClientMessage {
        crate::ServerToClientMessage::ChannelRemove(crate::raw::ChannelRemove { channel_id })
    }

    // Observed from a real server: it moves everyone out of a channel (to its
    // parent), then sends ChannelRemove last. So the removal never strands users.
    #[test]
    fn removing_a_channel_after_the_server_moves_its_users_out() {
        let mut state = MumbleState::default();
        state.apply(channel_state(0, "Root", None)).unwrap();
        state.apply(channel_state(1, "Another channel", Some(0))).unwrap();
        state.apply(user_in_channel(5, 1)).unwrap();

        state.apply(user_in_channel(5, 0)).unwrap();
        let event = state.apply(channel_remove(1)).unwrap();

        assert!(matches!(event, Some(MumbleEvent::ChannelsChanged)), "{event:?}");
        assert_eq!(state.channels().iter().map(|c| c.channel_id).collect::<Vec<_>>(), [0]);
        assert_eq!(state.users[&5].channel_id, 0);
    }

    #[test]
    fn the_current_user_follows_their_channel_being_removed() {
        let mut state = MumbleState::default();
        state.apply(channel_state(0, "Root", None)).unwrap();
        state.apply(channel_state(1, "Another channel", Some(0))).unwrap();
        state.apply(user_in_channel(22, 1)).unwrap();
        state.apply(server_sync(Some(22))).unwrap();

        state.apply(user_in_channel(22, 0)).unwrap();
        state.apply(channel_remove(1)).unwrap();

        assert_eq!(state.current_user().unwrap().channel_id, 0);
        assert_eq!(me(&state), [22]);
    }

    #[test]
    fn users_can_move_between_known_channels() {
        let mut state = MumbleState::default();
        state.apply(channel_state(1, "Lobby", Some(0))).unwrap();
        state.apply(channel_state(2, "Games", Some(0))).unwrap();
        state.apply(user_in_channel(1, 1)).unwrap();

        let event = state.apply(user_in_channel(1, 2)).unwrap();

        assert!(matches!(event, Some(MumbleEvent::UsersChanged)), "{event:?}");
        assert_eq!(state.users[&1].channel_id, 2);
    }

    #[test]
    fn putting_a_user_in_an_unknown_channel_is_an_error() {
        let mut state = MumbleState::default();
        state.apply(channel_state(1, "Lobby", Some(0))).unwrap();

        let error = state.apply(user_in_channel(1, 42)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("42"), "{error}");
    }

    #[test]
    fn moving_a_user_to_an_unknown_channel_leaves_them_where_they_were() {
        let mut state = MumbleState::default();
        state.apply(channel_state(1, "Lobby", Some(0))).unwrap();
        state.apply(user_in_channel(1, 1)).unwrap();

        state.apply(user_in_channel(1, 42)).unwrap_err();

        assert_eq!(state.users[&1].channel_id, 1);
    }

    // The server omits unchanged fields, so most user updates have no channel.
    #[test]
    fn user_updates_without_a_channel_are_fine() {
        let mut state = MumbleState::default();

        state.apply(user_state(1, "Alice")).unwrap();
    }

    #[test]
    fn there_is_no_current_user_before_the_server_sync() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();

        assert!(state.current_user().is_none());
        assert!(me(&state).is_empty());
    }

    #[test]
    fn the_server_sync_picks_out_the_current_user() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();
        state.apply(user_state(2, "Bob")).unwrap();

        state.apply(server_sync(Some(2))).unwrap();

        assert_eq!(state.current_user().map(|u| u.name.as_str()), Some("Bob"));
        assert_eq!(me(&state), [2]);
    }

    // The server sends our own UserState before the ServerSync, but nothing
    // in the types guarantees it.
    #[test]
    fn the_current_user_is_marked_even_if_they_arrive_after_the_server_sync() {
        let mut state = MumbleState::default();
        state.apply(server_sync(Some(2))).unwrap();

        state.apply(user_state(2, "Bob")).unwrap();

        assert_eq!(state.current_user().map(|u| u.name.as_str()), Some("Bob"));
        assert_eq!(me(&state), [2]);
    }

    #[test]
    fn user_updates_keep_the_current_user_marked() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();
        state.apply(server_sync(Some(1))).unwrap();

        state.apply(user_state(1, "Alice Cooper")).unwrap();

        assert_eq!(me(&state), [1]);
    }

    #[test]
    fn the_current_user_follows_their_channel() {
        let mut state = MumbleState::default();
        state.apply(channel_state(1, "Lobby", Some(0))).unwrap();
        state.apply(channel_state(2, "Games", Some(0))).unwrap();
        state.apply(user_in_channel(1, 1)).unwrap();
        state.apply(server_sync(Some(1))).unwrap();

        state.apply(user_in_channel(1, 2)).unwrap();

        assert_eq!(state.current_user().unwrap().channel_id, 2);
    }

    // Mumble sends one ServerSync per connection, to finish the handshake. A
    // second one means we've lost track of the protocol, and silently
    // re-pointing "me" would leave two users flagged.
    #[test]
    fn a_second_server_sync_is_an_error() {
        let mut state = MumbleState::default();
        state.apply(user_state(1, "Alice")).unwrap();
        state.apply(user_state(2, "Bob")).unwrap();
        state.apply(server_sync(Some(1))).unwrap();

        let error = state.apply(server_sync(Some(2))).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(me(&state), [1], "the original identity must survive");
    }
}
