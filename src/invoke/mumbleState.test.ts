// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { z } from "zod";
import { Channel, User } from "./mumbleState";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import fixture from "../../crates/mumble-protocol/fixtures/users.json";
import channelFixture from "../../crates/mumble-protocol/fixtures/channels.json";

describe("Channel", () => {
  it("parses Rust's channels", () => {
    const result = z.array(Channel).safeParse(channelFixture);
    expect(result.error?.issues).toBeUndefined();
  });

  it("keeps the root channel parentless", () => {
    const [root, lobby] = z.array(Channel).parse(channelFixture);
    expect(root.parent).toBeNull();
    expect(lobby.parent).toBe(0);
  });

  it("rejects fields the schema doesn't know about", () => {
    expect(Channel.safeParse({ ...channelFixture[1], new_field: 1 }).success).toBe(false);
  });

  it("rejects a negative channel id", () => {
    expect(Channel.safeParse({ ...channelFixture[1], channel_id: -1 }).success).toBe(false);
  });
});

describe("User identity", () => {
  it("requires the is_me flag", () => {
    const { is_me: _isMe, ...user } = fixture[0];
    expect(User.safeParse(user).success).toBe(false);
  });
});

describe("User", () => {
  it("parses Rust's users", () => {
    const result = z.array(User).safeParse(fixture);
    expect(result.error?.issues).toBeUndefined();
  });

  it("rejects fields the schema doesn't know about", () => {
    const user = { ...fixture[0], new_field: 1 };
    expect(User.safeParse(user).success).toBe(false);
  });

  it("requires the speaking flag", () => {
    const { is_speaking: _isSpeaking, ...user } = fixture[0];
    expect(User.safeParse(user).success).toBe(false);
  });

  it("rejects missing fields", () => {
    const { name: _name, ...user } = fixture[0];
    expect(User.safeParse(user).success).toBe(false);
  });
});
