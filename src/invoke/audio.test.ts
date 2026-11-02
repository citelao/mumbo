// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import fixture from "../../src-tauri/fixtures/audio_devices.json";
import { emit, listen } from "@tauri-apps/api/event";
import {
    setAudioInputDevice,
    setAudioOutputDevice,
    setNoiseSuppression,
    subscribeAudioDevices,
} from "./audio";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(), emit: vi.fn() }));
const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);
const mockEmit = vi.mocked(emit);

let listeners: Record<string, (event: { payload: unknown }) => void>;
const unlisten = vi.fn();

beforeEach(() => {
    mockInvoke.mockReset();
    unlisten.mockReset();
    mockEmit.mockReset().mockResolvedValue(undefined);
    listeners = {};
    mockListen.mockReset().mockImplementation((async (name: string, callback: (event: { payload: unknown }) => void) => {
        listeners[name] = callback;
        return unlisten;
    }) as unknown as typeof listen);
});

const devices = [
    { id: "a", name: "Built-in", is_default: true },
    { id: "b", name: "USB", is_default: false },
];

describe("subscribeAudioDevices", () => {
    const setup = async () => {
        const onInputs = vi.fn();
        const onOutputs = vi.fn();
        const onInputSelected = vi.fn();
        const onOutputSelected = vi.fn();
        const onNoiseSuppression = vi.fn();
        const stop = await subscribeAudioDevices({ onInputs, onOutputs, onInputSelected, onOutputSelected, onNoiseSuppression });
        return { onInputs, onOutputs, onInputSelected, onOutputSelected, onNoiseSuppression, stop };
    };
    const send = (event: string, payload: unknown) => listeners[event]!({ payload });

    it("registers every listener before announcing itself", async () => {
        await setup();
        expect(Object.keys(listeners).sort()).toEqual([
            "audio-input-device-selected",
            "audio-input-devices-changed",
            "audio-noise-suppression-changed",
            "audio-output-device-selected",
            "audio-output-devices-changed",
        ]);
        // The backend only publishes on announce; an earlier emit would be lost.
        expect(mockEmit).toHaveBeenCalledWith("audio-devices-subscribe");
        expect(mockEmit.mock.invocationCallOrder[0]).toBeGreaterThan(
            Math.max(...mockListen.mock.invocationCallOrder),
        );
    });

    it("parses the exact JSON Rust emits", async () => {
        const { onInputs } = await setup();
        send("audio-input-devices-changed", fixture);
        expect(onInputs).toHaveBeenCalledWith(fixture);
    });

    it("sends each list to its own handler", async () => {
        const { onInputs, onOutputs } = await setup();
        send("audio-input-devices-changed", devices);
        send("audio-output-devices-changed", [devices[1]]);
        expect(onInputs).toHaveBeenCalledExactlyOnceWith(devices);
        expect(onOutputs).toHaveBeenCalledExactlyOnceWith([devices[1]]);
    });

    it("accepts an empty list", async () => {
        const { onOutputs } = await setup();
        send("audio-output-devices-changed", []);
        expect(onOutputs).toHaveBeenCalledWith([]);
    });

    it.each([
        ["missing is_default", [{ id: "a", name: "x" }]],
        ["non-string id", [{ id: 1, name: "x", is_default: false }]],
        ["non-array payload", { id: "a" }],
        ["null payload", null],
    ])("logs and ignores malformed payloads: %s", async (_name, payload) => {
        const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
        const { onInputs } = await setup();
        send("audio-input-devices-changed", payload);
        expect(onInputs).not.toHaveBeenCalled();
        expect(consoleError).toHaveBeenCalled();
        consoleError.mockRestore();
    });

    it("sends the selected device's id, or null for the system default", async () => {
        const { onInputSelected, onOutputSelected } = await setup();
        send("audio-input-device-selected", devices[1]);
        send("audio-input-device-selected", null);
        send("audio-output-device-selected", devices[0]);
        expect(onInputSelected.mock.calls).toEqual([["b"], [null]]);
        expect(onOutputSelected.mock.calls).toEqual([["a"]]);
    });

    it.each([
        ["missing is_default", { id: "a", name: "x" }],
        ["a bare id", "a"],
        ["a list", [devices[0]]],
    ])("logs and ignores a malformed selection: %s", async (_name, payload) => {
        const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
        const { onInputSelected } = await setup();
        send("audio-input-device-selected", payload);
        expect(onInputSelected).not.toHaveBeenCalled();
        expect(consoleError).toHaveBeenCalled();
        consoleError.mockRestore();
    });

    it("sends whether noise suppression is on", async () => {
        const { onNoiseSuppression } = await setup();
        send("audio-noise-suppression-changed", false);
        send("audio-noise-suppression-changed", true);
        expect(onNoiseSuppression.mock.calls).toEqual([[false], [true]]);
    });

    it.each([
        ["a string", "yes"],
        ["a number", 1],
        ["null", null],
        ["an object", { enabled: true }],
    ])("logs and ignores a malformed noise suppression setting: %s", async (_name, payload) => {
        const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
        const { onNoiseSuppression } = await setup();
        send("audio-noise-suppression-changed", payload);
        expect(onNoiseSuppression).not.toHaveBeenCalled();
        expect(consoleError).toHaveBeenCalled();
        consoleError.mockRestore();
    });

    it("stops every listener when unsubscribed", async () => {
        const { stop } = await setup();
        expect(unlisten).not.toHaveBeenCalled();
        stop();
        expect(unlisten).toHaveBeenCalledTimes(5);
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

    it("passes null when going back to the default device", async () => {
        mockInvoke.mockResolvedValue(undefined);
        await fn(null);
        expect(mockInvoke).toHaveBeenCalledWith(command, { deviceId: null });
    });

    it("propagates backend errors", async () => {
        mockInvoke.mockRejectedValue("unknown device");
        await expect(fn("zzz")).rejects.toBe("unknown device");
    });
});

describe("set noise suppression", () => {
    it.each([true, false])("tells the backend %s", async (enabled) => {
        mockInvoke.mockResolvedValue(undefined);
        await setNoiseSuppression(enabled);
        expect(mockInvoke).toHaveBeenCalledWith("set_noise_suppression", { enabled });
    });

    it("propagates backend errors", async () => {
        mockInvoke.mockRejectedValue("no audio module");
        await expect(setNoiseSuppression(false)).rejects.toBe("no audio module");
    });
});
