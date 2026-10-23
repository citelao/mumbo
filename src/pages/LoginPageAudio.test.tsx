// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import LoginPage from "./LoginPage";
import * as audio from "../invoke/audio";

vi.mock("../invoke", async (orig) => ({
    ...(await orig<typeof import("../invoke")>()),
    getServerStats: vi.fn(async () => { throw new Error("offline"); }),
}));
vi.mock("../invoke/audio", async (orig) => ({
    ...(await orig<typeof import("../invoke/audio")>()),
    subscribeAudioDevices: vi.fn(),
    setAudioInputDevice: vi.fn(),
    setAudioOutputDevice: vi.fn(),
}));

const inputs = [
    { id: "in-a", name: "Built-in Mic", is_default: false },
    { id: "in-b", name: "USB Mic", is_default: true },
];
const outputs = [
    { id: "out-a", name: "Speakers", is_default: true },
    { id: "out-b", name: "Headphones", is_default: false },
];

let handlers: audio.AudioDeviceHandlers;
const stopListening = vi.fn();

beforeEach(() => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    stopListening.mockReset();
    vi.mocked(audio.setAudioInputDevice).mockReset().mockResolvedValue(undefined);
    vi.mocked(audio.setAudioOutputDevice).mockReset().mockResolvedValue(undefined);
    vi.mocked(audio.subscribeAudioDevices).mockImplementation(async (h) => {
        handlers = h;
        return stopListening;
    });
});
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const select = (label: string) => screen.getByLabelText(label) as HTMLSelectElement;
const renderPage = () => render(<LoginPage onConnect={vi.fn()} onRemoveRecentServer={vi.fn()} />);
// Renders, then plays the backend's initial device lists.
const renderWithDevices = async (ins = inputs, outs = outputs) => {
    const view = renderPage();
    await waitFor(() => expect(audio.subscribeAudioDevices).toHaveBeenCalled());
    act(() => {
        handlers.onInputs(ins);
        handlers.onOutputs(outs);
    });
    return view;
};

describe("LoginPage audio settings", () => {
    it("lists devices and labels the default", async () => {
        await renderWithDevices();
        await screen.findByRole("option", { name: /USB Mic/ });
        expect(screen.getByRole("option", { name: "USB Mic (Default)" })).toBeTruthy();
        expect(screen.getByRole("option", { name: "Built-in Mic" })).toBeTruthy();
        expect(screen.getByRole("option", { name: "Headphones" })).toBeTruthy();
    });

    it("preselects the default input and output", async () => {
        await renderWithDevices();
        await waitFor(() => expect(select("Audio input").value).toBe("in-b"));
        expect(select("Audio output").value).toBe("out-a");
    });

    it("changing the selection updates the select", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await waitFor(() => expect(select("Audio input").value).toBe("in-b"));
        await user.selectOptions(select("Audio input"), "in-a");
        await user.selectOptions(select("Audio output"), "out-b");
        expect(select("Audio input").value).toBe("in-a");
        expect(select("Audio output").value).toBe("out-b");
    });

    it("tells the backend which input the user picks", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio input"), "in-a");
        expect(audio.setAudioInputDevice).toHaveBeenCalledExactlyOnceWith("in-a");
        expect(audio.setAudioOutputDevice).not.toHaveBeenCalled();
    });

    it("tells the backend which output the user picks", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio output"), "out-b");
        expect(audio.setAudioOutputDevice).toHaveBeenCalledExactlyOnceWith("out-b");
        expect(audio.setAudioInputDevice).not.toHaveBeenCalled();
    });

    it("does not report the automatic default selection as a user choice", async () => {
        await renderWithDevices();
        await waitFor(() => expect(select("Audio input").value).toBe("in-b"));
        expect(audio.setAudioInputDevice).not.toHaveBeenCalled();
        expect(audio.setAudioOutputDevice).not.toHaveBeenCalled();
    });

    it("logs when the backend rejects the choice", async () => {
        vi.mocked(audio.setAudioInputDevice).mockRejectedValue("Unknown audio device: in-a");
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio input"), "in-a");
        // The offline server stats also log, so match this failure specifically.
        await waitFor(() => expect(console.error).toHaveBeenCalledWith(
            "Failed to set audio input device:", "Unknown audio device: in-a",
        ));
    });

    it("handles no devices without crashing", async () => {
        await renderWithDevices([], []);
        expect(select("Audio input").options).toHaveLength(0);
        expect(select("Audio input").value).toBe("");
    });

    it("selects nothing when no device is flagged default", async () => {
        await renderWithDevices(inputs.map((d) => ({ ...d, is_default: false })));
        await screen.findByRole("option", { name: "USB Mic" });
        // A controlled <select> with value "" and no matching option shows the first option.
        expect(select("Audio input").selectedOptions[0]?.value).toBe("in-a");
    });

    it("keeps the user's pick when the list changes", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio input"), "in-a");

        act(() => handlers.onInputs([...inputs, { id: "in-c", name: "Dock Mic", is_default: false }]));

        await screen.findByRole("option", { name: "Dock Mic" });
        expect(select("Audio input").value).toBe("in-a");
    });

    it("falls back to the default when the selected device is unplugged", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio input"), "in-a");

        act(() => handlers.onInputs([inputs[1]!]));

        await waitFor(() => expect(select("Audio input").value).toBe("in-b"));
        expect(screen.queryByRole("option", { name: "Built-in Mic" })).toBeNull();
    });

    it("shows empty selects and logs when subscribing fails", async () => {
        vi.mocked(audio.subscribeAudioDevices).mockRejectedValue("boom");
        renderPage();
        await waitFor(() => expect(console.error).toHaveBeenCalled());
        expect(select("Audio input").options).toHaveLength(0);
        expect(select("Audio output").options).toHaveLength(0);
        expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
    });

    it("stops listening when the page unmounts", async () => {
        const { unmount } = await renderWithDevices();
        unmount();
        await waitFor(() => expect(stopListening).toHaveBeenCalledOnce());
    });
});
