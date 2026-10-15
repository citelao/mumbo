import { describe, expect, it } from "vitest";
import { ServerToClientMessage } from "./protocol";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import fixture from "../crates/mumble-protocol/fixtures/server_to_client_messages.json";

describe("ServerToClientMessage", () => {
  it.each(fixture.map((message) => [message.type, message]))("parses Rust's %s", (_type, message) => {
    const result = ServerToClientMessage.safeParse(message);
    expect(result.error?.issues).toBeUndefined();
  });

  it("covers every variant in the fixture", () => {
    const fixtureTypes = new Set(fixture.map((message) => message.type));
    const schemaTypes = ServerToClientMessage.options.map((option) => option.shape.type.value);
    expect(fixtureTypes).toEqual(new Set(schemaTypes));
  });

  it("rejects fields the schema doesn't know about", () => {
    const message = { type: "ChannelRemove", data: { channel_id: 0, new_field: 1 } };
    expect(ServerToClientMessage.safeParse(message).success).toBe(false);
  });

  it("rejects missing fields", () => {
    const message = { type: "ChannelRemove", data: {} };
    expect(ServerToClientMessage.safeParse(message).success).toBe(false);
  });

  it("rejects unknown message types", () => {
    const message = { type: "SomeNewMessage", data: {} };
    expect(ServerToClientMessage.safeParse(message).success).toBe(false);
  });
});
