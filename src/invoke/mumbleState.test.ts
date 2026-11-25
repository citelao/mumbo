// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { z } from "zod";
import { Channel, SdkUser, TextMessage, User } from "./mumbleState";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import fixture from "../../fixtures/mumbo-sdk/users.json";
import channelFixture from "../../fixtures/mumbo-sdk/channels.json";
import chatFixture from "../../fixtures/mumbo-sdk/chat_messages.json";

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

  it("flags channels that need a password", () => {
    const [root, lobby] = z.array(Channel).parse(channelFixture);
    expect(root.is_enter_restricted).toBe(false);
    expect(lobby.is_enter_restricted).toBe(true);
  });

  it.each(["is_enter_restricted", "can_enter"])("requires %s", (field) => {
    const { [field]: _omitted, ...channel } = channelFixture[1] as Record<string, unknown>;
    expect(Channel.safeParse(channel).success).toBe(false);
  });

  it("rejects a negative channel id", () => {
    expect(Channel.safeParse({ ...channelFixture[1], channel_id: -1 }).success).toBe(false);
  });
});

describe("SdkUser identity", () => {
  it("requires the is_me flag", () => {
    const { is_me: _isMe, ...user } = fixture[0];
    expect(SdkUser.safeParse(user).success).toBe(false);
  });
});

describe("SdkUser", () => {
  it("parses Rust's users", () => {
    const result = z.array(SdkUser).safeParse(fixture);
    expect(result.error?.issues).toBeUndefined();
  });

  it("rejects fields the schema doesn't know about", () => {
    const user = { ...fixture[0], new_field: 1 };
    expect(SdkUser.safeParse(user).success).toBe(false);
  });

  it("requires the speaking flag", () => {
    const { is_speaking: _isSpeaking, ...user } = fixture[0];
    expect(SdkUser.safeParse(user).success).toBe(false);
  });

  it("rejects missing fields", () => {
    const { name: _name, ...user } = fixture[0];
    expect(SdkUser.safeParse(user).success).toBe(false);
  });
});

describe("User", () => {
  it("parses an SDK user with a volume adjustment", () => {
    const result = User.safeParse({ ...fixture[0], volume_adjustment_db: -6 });
    expect(result.error?.issues).toBeUndefined();
  });

  it("parses an SDK user without a volume adjustment", () => {
    const result = User.safeParse({ ...fixture[0], volume_adjustment_db: null });
    expect(result.error?.issues).toBeUndefined();
  });

  it("requires the volume adjustment, since Rust sends None as null", () => {
    expect(User.safeParse(fixture[0]).success).toBe(false);
  });

  it("rejects fields the schema doesn't know about", () => {
    const user = { ...fixture[0], volume_adjustment_db: null, new_field: 1 };
    expect(User.safeParse(user).success).toBe(false);
  });
});

describe("TextMessage", () => {
  it("parses Rust's chat messages", () => {
    const result = z.array(TextMessage).safeParse(chatFixture);
    expect(result.error?.issues).toBeUndefined();
  });

  it("covers every variant in the fixture", () => {
    const fixtureTypes = new Set(chatFixture.map((message) => message.type));
    const schemaTypes = TextMessage.options.map((option) => option.shape.type.value);
    expect(fixtureTypes).toEqual(new Set(schemaTypes));
  });
});
