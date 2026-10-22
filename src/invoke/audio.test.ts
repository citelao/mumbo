// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import fixture from "../../src-tauri/fixtures/audio_devices.json";
import {
    getAudioInputDevices,
    getAudioOutputDevices,
    setAudioInputDevice,
    setAudioOutputDevice,
} from "./audio";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
    mockInvoke.mockReset();
});

const devices = [
    { id: "a", name: "Built-in", is_default: true },
    { id: "b", name: "USB", is_default: false },
];

for (const [kind, fn, command] of [
    ["input", getAudioInputDevices, "get_audio_input_devices"],
    ["output", getAudioOutputDevices, "get_audio_output_devices"],
] as const) describe(`get audio ${kind} devices`, () => {
    it("parses the exact JSON Rust emits", async () => {
        mockInvoke.mockResolvedValue(fixture);
        expect(await fn()).toEqual(fixture);
    });

    it("invokes the right command and returns parsed devices", async () => {
        mockInvoke.mockResolvedValue(devices);
        expect(await fn()).toEqual(devices);
        expect(mockInvoke).toHaveBeenCalledWith(command);
    });

    it("accepts an empty list", async () => {
        mockInvoke.mockResolvedValue([]);
        expect(await fn()).toEqual([]);
    });

    it.each([
        ["missing is_default", [{ id: "a", name: "x" }]],
        ["non-string id", [{ id: 1, name: "x", is_default: false }]],
        ["non-array payload", { id: "a" }],
        ["null payload", null],
    ])("rejects malformed payloads: %s", async (_name, payload) => {
        mockInvoke.mockResolvedValue(payload);
        await expect(fn()).rejects.toThrow();
    });

    it("propagates backend errors", async () => {
        mockInvoke.mockRejectedValue("no audio host");
        await expect(fn()).rejects.toBe("no audio host");
    });
});

for (const [kind, fn, command] of [
    ["input", setAudioInputDevice, "set_audio_input_device"],
    ["output", setAudioOutputDevice, "set_audio_output_device"],
] as const) describe(`set audio ${kind} device`, () => {
    it("passes the device id as camelCase deviceId", async () => {
        mockInvoke.mockResolvedValue(undefined);
        await fn("b");
        expect(mockInvoke).toHaveBeenCalledWith(command, { deviceId: "b" });
    });

    it("propagates backend errors", async () => {
        mockInvoke.mockRejectedValue("unknown device");
        await expect(fn("zzz")).rejects.toBe("unknown device");
    });
});
