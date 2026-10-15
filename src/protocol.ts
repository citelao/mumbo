import { z } from "zod";

// Mirrors `mumble_protocol::ServerToClientMessage` as serialized by serde.
// Rust's `Option<T>` serializes as `null` (not a missing key), so optional
// proto fields are `.nullable()`. Objects are strict so that fields added or
// renamed on the Rust side fail parsing instead of silently passing through.

const u32 = z.number().int().nonnegative();
// JSON numbers are f64: u64 values above 2^53 lose precision in transit.
const u64 = z.number().int().nonnegative();
const i32 = z.number().int();
const bytes = z.array(z.number().int().min(0).max(255));

const Version = z.strictObject({
  version_v1: u32.nullable(),
  version_v2: u64.nullable(),
  release: z.string().nullable(),
  os: z.string().nullable(),
  os_version: z.string().nullable(),
});

const Reject = z.strictObject({
  type: i32.nullable(),
  reason: z.string().nullable(),
});

const ServerSync = z.strictObject({
  session: u32.nullable(),
  max_bandwidth: u32.nullable(),
  welcome_text: z.string().nullable(),
  permissions: u64.nullable(),
});

const ChannelRemove = z.strictObject({
  channel_id: u32,
});

const ChannelState = z.strictObject({
  channel_id: u32.nullable(),
  parent: u32.nullable(),
  name: z.string().nullable(),
  links: z.array(u32),
  description: z.string().nullable(),
  links_add: z.array(u32),
  links_remove: z.array(u32),
  temporary: z.boolean().nullable(),
  position: i32.nullable(),
  description_hash: bytes.nullable(),
  max_users: u32.nullable(),
  is_enter_restricted: z.boolean().nullable(),
  can_enter: z.boolean().nullable(),
});

const UserRemove = z.strictObject({
  session: u32,
  actor: u32.nullable(),
  reason: z.string().nullable(),
  ban: z.boolean().nullable(),
});

const VolumeAdjustment = z.strictObject({
  listening_channel: u32.nullable(),
  volume_adjustment: z.number().nullable(),
});

const UserState = z.strictObject({
  session: u32.nullable(),
  actor: u32.nullable(),
  name: z.string().nullable(),
  user_id: u32.nullable(),
  channel_id: u32.nullable(),
  mute: z.boolean().nullable(),
  deaf: z.boolean().nullable(),
  suppress: z.boolean().nullable(),
  self_mute: z.boolean().nullable(),
  self_deaf: z.boolean().nullable(),
  texture: bytes.nullable(),
  plugin_context: bytes.nullable(),
  plugin_identity: z.string().nullable(),
  comment: z.string().nullable(),
  hash: z.string().nullable(),
  comment_hash: bytes.nullable(),
  texture_hash: bytes.nullable(),
  priority_speaker: z.boolean().nullable(),
  recording: z.boolean().nullable(),
  temporary_access_tokens: z.array(z.string()),
  listening_channel_add: z.array(u32),
  listening_channel_remove: z.array(u32),
  listening_volume_adjustment: z.array(VolumeAdjustment),
});

const CryptSetup = z.strictObject({
  key: bytes.nullable(),
  client_nonce: bytes.nullable(),
  server_nonce: bytes.nullable(),
});

const PermissionQuery = z.strictObject({
  channel_id: u32.nullable(),
  permissions: u32.nullable(),
  flush: z.boolean().nullable(),
});

const CodecVersion = z.strictObject({
  alpha: i32,
  beta: i32,
  prefer_alpha: z.boolean(),
  opus: z.boolean().nullable(),
});

const ServerConfig = z.strictObject({
  max_bandwidth: u32.nullable(),
  welcome_text: z.string().nullable(),
  allow_html: z.boolean().nullable(),
  message_length: u32.nullable(),
  image_message_length: u32.nullable(),
  max_users: u32.nullable(),
  recording_allowed: z.boolean().nullable(),
});

// `audio::Header` is a protobuf oneof, which serde tags externally.
const AudioHeader = z.union([
  z.strictObject({ Target: u32 }),
  z.strictObject({ Context: u32 }),
]);

const UdpAudio = z.strictObject({
  sender_session: u32,
  frame_number: u64,
  opus_data: bytes,
  positional_data: z.array(z.number()),
  volume_adjustment: z.number(),
  is_terminator: z.boolean(),
  header: AudioHeader.nullable(),
});

const UdpPing = z.strictObject({
  timestamp: u64,
  request_extended_information: z.boolean(),
  server_version_v2: u64,
  user_count: u32,
  max_user_count: u32,
  max_bandwidth_per_user: u32,
});

const ServerToClientUdpMessage = z.discriminatedUnion("type", [
  z.strictObject({ type: z.literal("Audio"), data: UdpAudio }),
  z.strictObject({ type: z.literal("Ping"), data: UdpPing }),
]);

export const ServerToClientMessage = z.discriminatedUnion("type", [
  z.strictObject({ type: z.literal("Version"), data: Version }),
  z.strictObject({ type: z.literal("UdpTunnel"), data: ServerToClientUdpMessage }),
  z.strictObject({ type: z.literal("Reject"), data: Reject }),
  z.strictObject({ type: z.literal("ServerSync"), data: ServerSync }),
  z.strictObject({ type: z.literal("ChannelRemove"), data: ChannelRemove }),
  z.strictObject({ type: z.literal("ChannelState"), data: ChannelState }),
  z.strictObject({ type: z.literal("UserRemove"), data: UserRemove }),
  z.strictObject({ type: z.literal("UserState"), data: UserState }),
  z.strictObject({ type: z.literal("CryptSetup"), data: CryptSetup }),
  z.strictObject({ type: z.literal("PermissionQuery"), data: PermissionQuery }),
  z.strictObject({ type: z.literal("CodecVersion"), data: CodecVersion }),
  z.strictObject({ type: z.literal("ServerConfig"), data: ServerConfig }),
]);

export type ServerToClientMessage = z.infer<typeof ServerToClientMessage>;
