import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { fetchChatMessages, fetchUsers } from "./mumbleState";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import fixture from "../crates/mumble-protocol/fixtures/users.json";

// There's no Tauri runtime under vitest.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockReset();
});

describe("fetchUsers", () => {
  it("asks Rust for users and returns them parsed", async () => {
    mockInvoke.mockResolvedValue(fixture);
    await expect(fetchUsers()).resolves.toEqual(fixture);
    expect(mockInvoke).toHaveBeenCalledWith("get_users");
  });

  it("rejects users with fields the schema doesn't know about", async () => {
    mockInvoke.mockResolvedValue([{ ...fixture[0], new_field: 1 }]);
    await expect(fetchUsers()).rejects.toThrow();
  });

  it("rejects users with mistyped fields", async () => {
    mockInvoke.mockResolvedValue([{ ...fixture[0], session: "1" }]);
    await expect(fetchUsers()).rejects.toThrow();
  });

  it("rejects a non-array response", async () => {
    mockInvoke.mockResolvedValue(fixture[0]);
    await expect(fetchUsers()).rejects.toThrow();
  });

  it("propagates errors from Rust", async () => {
    mockInvoke.mockRejectedValue("not connected");
    await expect(fetchUsers()).rejects.toBe("not connected");
  });
});

describe("fetchChatMessages", () => {
  // Shaped like serde's output for `mumble_protocol::state::TextMessage`.
  const messages = [
    { type: "ServerTextMessage", timestamp: "2026-10-19T05:31:31.123456Z", content: "Welcome!" },
    { type: "ChatTextMessage", timestamp: "2026-10-19T05:31:32Z", user_session: 5, content: "hi" },
    { type: "ChatTextMessage", timestamp: "2026-10-19T05:31:33Z", user_session: null, content: "who said this?" },
  ];

  it("asks Rust for chat messages and returns them in order", async () => {
    mockInvoke.mockResolvedValue(messages);
    await expect(fetchChatMessages()).resolves.toEqual(messages);
    expect(mockInvoke).toHaveBeenCalledWith("get_chat_messages");
  });

  it("rejects messages with fields the schema doesn't know about", async () => {
    mockInvoke.mockResolvedValue([{ ...messages[1], channel_id: 1 }]);
    await expect(fetchChatMessages()).rejects.toThrow();
  });

  it("rejects messages of an unknown type", async () => {
    mockInvoke.mockResolvedValue([{ ...messages[0], type: "DirectTextMessage" }]);
    await expect(fetchChatMessages()).rejects.toThrow();
  });
});
