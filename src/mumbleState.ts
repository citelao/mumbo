import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";

// Mirrors `mumble_protocol::state::User` as serialized by serde. Strict, so
// fields added or renamed on the Rust side fail parsing instead of passing
// through unnoticed.
export const User = z.strictObject({
  session: z.number().int().nonnegative(),
  name: z.string(),
  channel_id: z.number().int().nonnegative(),
  mute: z.boolean(),
  deaf: z.boolean(),
  suppress: z.boolean(),
  self_mute: z.boolean(),
  self_deaf: z.boolean(),
  priority_speaker: z.boolean(),
  recording: z.boolean(),
});

export type User = z.infer<typeof User>;

export async function fetchUsers(): Promise<User[]> {
  // `invoke` returns whatever Rust sent; parse rather than trust the cast.
  return z.array(User).parse(await invoke<unknown>("get_users"));
}

// Mirrors `mumble_protocol::state::TextMessage` as serialized by serde: an
// internally tagged enum, with `Option` fields sent as `null` and `DateTime`s
// as RFC 3339 strings.
export const TextMessage = z.discriminatedUnion("type", [
  z.strictObject({
    type: z.literal("ChatTextMessage"),
    timestamp: z.string(),
    user_session: z.number().int().nonnegative().nullable(),
    content: z.string(),
  }),
  z.strictObject({
    type: z.literal("ServerTextMessage"),
    timestamp: z.string(),
    content: z.string(),
  }),
]);

export type TextMessage = z.infer<typeof TextMessage>;

export async function fetchChatMessages(): Promise<TextMessage[]> {
  return z.array(TextMessage).parse(await invoke<unknown>("get_chat_messages"));
}