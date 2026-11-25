// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { onDisconnected } from "./connection";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
const mockListen = vi.mocked(listen);

/** Delivers a "disconnected" event from Rust with `payload`. */
function emitDisconnected(payload: unknown) {
    const [event, handler] = mockListen.mock.calls[0];
    expect(event).toBe("disconnected");
    handler({ event, id: 0, payload });
}

beforeEach(() => {
    mockListen.mockClear();
});

describe("onDisconnected", () => {
    // The shapes Rust's `disconnect_reasons_serialize_as_tagged_objects` test pins.
    it.each([
        ["a removal", { kind: "removed", reason: "go away", by: null, banned: true }],
        ["a clean close", { kind: "server_closed" }],
        ["a lost connection", { kind: "connection_lost", detail: "IO error" }],
    ])("passes on %s", async (_, reason) => {
        const handler = vi.fn();
        await onDisconnected(handler);

        emitDisconnected({ reason });

        expect(handler).toHaveBeenCalledExactlyOnceWith(reason);
    });

    it.each([
        ["an unknown kind", { reason: { kind: "exploded" } }],
        ["a removal missing a field", { reason: { kind: "removed", reason: null, by: null } }],
        ["a lost connection without detail", { reason: { kind: "connection_lost" } }],
        ["the old string reason", { reason: "peer closed connection" }],
        ["an unknown field", { reason: { kind: "server_closed" }, new_field: 1 }],
    ])("logs and drops an event with %s", async (_, payload) => {
        const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
        const handler = vi.fn();
        await onDisconnected(handler);

        emitDisconnected(payload);

        expect(handler).not.toHaveBeenCalled();
        expect(consoleError).toHaveBeenCalled();
        consoleError.mockRestore();
    });
});
