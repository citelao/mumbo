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
  it("asks Rust for chat messages and returns them in order", async () => {
    mockInvoke.mockResolvedValue(["Welcome!", "hi"]);
    await expect(fetchChatMessages()).resolves.toEqual(["Welcome!", "hi"]);
    expect(mockInvoke).toHaveBeenCalledWith("get_chat_messages");
  });

  it("rejects messages that aren't plain strings, e.g. if Rust adds metadata", async () => {
    mockInvoke.mockResolvedValue([{ actor: 5, message: "hi" }]);
    await expect(fetchChatMessages()).rejects.toThrow();
  });
});
