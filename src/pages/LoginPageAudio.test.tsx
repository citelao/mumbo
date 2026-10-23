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
    setNoiseSuppression: vi.fn(),
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
    vi.mocked(audio.setNoiseSuppression).mockReset().mockResolvedValue(undefined);
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
        expect(screen.getAllByRole("option", { name: "Default device" })).toHaveLength(2);
        expect(screen.getByRole("option", { name: "USB Mic (Default)" })).toBeTruthy();
        expect(screen.getByRole("option", { name: "Built-in Mic" })).toBeTruthy();
        expect(screen.getByRole("option", { name: "Headphones" })).toBeTruthy();
    });

    it("starts on \"Default device\" for both", async () => {
        await renderWithDevices();
        await screen.findByRole("option", { name: "USB Mic (Default)" });
        expect(select("Audio input").value).toBe("");
        expect(select("Audio output").value).toBe("");
        expect(select("Audio input").selectedOptions[0]?.textContent).toBe("Default device");
    });

    it("shows the device the backend already prefers", async () => {
        await renderWithDevices();
        act(() => {
            handlers.onInputSelected("in-a");
            handlers.onOutputSelected("out-b");
        });
        await waitFor(() => expect(select("Audio input").value).toBe("in-a"));
        expect(select("Audio output").value).toBe("out-b");
    });

    it("shows the preference again after coming back to the page", async () => {
        const first = await renderWithDevices();
        first.unmount();

        // The backend keeps the preference while the page is gone, and republishes it on subscribing.
        await renderWithDevices();
        act(() => handlers.onInputSelected("in-a"));

        await waitFor(() => expect(select("Audio input").value).toBe("in-a"));
        expect(audio.setAudioInputDevice).not.toHaveBeenCalled();
    });

    it("follows the backend back to the default device", async () => {
        await renderWithDevices();
        act(() => handlers.onInputSelected("in-a"));
        await waitFor(() => expect(select("Audio input").value).toBe("in-a"));

        act(() => handlers.onInputSelected(null));

        await waitFor(() => expect(select("Audio input").value).toBe(""));
    });

    it("changing the selection updates the select", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
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

    it("does not report the initial selection as a user choice", async () => {
        await renderWithDevices();
        await screen.findByRole("option", { name: "USB Mic (Default)" });
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

    it("goes back to the default input when the user picks \"Default device\"", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio input"), "in-a");

        await user.selectOptions(select("Audio input"), "");

        expect(select("Audio input").value).toBe("");
        expect(audio.setAudioInputDevice).toHaveBeenLastCalledWith(null);
        expect(audio.setAudioInputDevice).toHaveBeenCalledTimes(2);
    });

    it("goes back to the default output when the user picks \"Default device\"", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio output"), "out-b");

        await user.selectOptions(select("Audio output"), "");

        expect(select("Audio output").value).toBe("");
        expect(audio.setAudioOutputDevice).toHaveBeenLastCalledWith(null);
        expect(audio.setAudioInputDevice).not.toHaveBeenCalled();
    });

    it("handles no devices without crashing", async () => {
        await renderWithDevices([], []);
        // Only "Default device" is left to choose.
        expect(select("Audio input").options).toHaveLength(1);
        expect(select("Audio input").value).toBe("");
    });

    it("stays on \"Default device\" when no device is flagged default", async () => {
        await renderWithDevices(inputs.map((d) => ({ ...d, is_default: false })));
        await screen.findByRole("option", { name: "USB Mic" });
        expect(select("Audio input").value).toBe("");
    });

    it("keeps the user's pick when the list changes", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio input"), "in-a");

        act(() => handlers.onInputs([...inputs, { id: "in-c", name: "Dock Mic", is_default: false }]));

        await screen.findByRole("option", { name: "Dock Mic" });
        expect(select("Audio input").value).toBe("in-a");
    });

    it("shows \"Default device\" while the selected device is unplugged, and the device again when it returns", async () => {
        const user = userEvent.setup();
        await renderWithDevices();
        await user.selectOptions(select("Audio input"), "in-a");

        act(() => handlers.onInputs([inputs[1]!]));
        await waitFor(() => expect(select("Audio input").value).toBe(""));
        expect(screen.queryByRole("option", { name: "Built-in Mic" })).toBeNull();
        // Unplugging is not a choice: the backend keeps its preference, so the UI tells it nothing.
        expect(audio.setAudioInputDevice).toHaveBeenCalledExactlyOnceWith("in-a");

        act(() => handlers.onInputs(inputs));
        await waitFor(() => expect(select("Audio input").value).toBe("in-a"));
    });

    it("shows empty selects and logs when subscribing fails", async () => {
        vi.mocked(audio.subscribeAudioDevices).mockRejectedValue("boom");
        renderPage();
        await waitFor(() => expect(console.error).toHaveBeenCalled());
        expect(select("Audio input").options).toHaveLength(1);
        expect(select("Audio output").options).toHaveLength(1);
        expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
    });

    it("stops listening when the page unmounts", async () => {
        const { unmount } = await renderWithDevices();
        unmount();
        await waitFor(() => expect(stopListening).toHaveBeenCalledOnce());
    });

    describe("noise suppression", () => {
        const checkbox = () => screen.getByRole("checkbox", { name: "Noise suppression" }) as HTMLInputElement;

        it("is on until the backend says otherwise", async () => {
            await renderWithDevices();
            expect(checkbox().checked).toBe(true);
        });

        it("shows the backend's setting", async () => {
            await renderWithDevices();
            act(() => handlers.onNoiseSuppression(false));
            await waitFor(() => expect(checkbox().checked).toBe(false));

            act(() => handlers.onNoiseSuppression(true));
            await waitFor(() => expect(checkbox().checked).toBe(true));
        });

        it("tells the backend when the user turns it off and on again", async () => {
            const user = userEvent.setup();
            await renderWithDevices();

            await user.click(checkbox());
            expect(checkbox().checked).toBe(false);
            expect(audio.setNoiseSuppression).toHaveBeenLastCalledWith(false);

            await user.click(checkbox());
            expect(checkbox().checked).toBe(true);
            expect(audio.setNoiseSuppression).toHaveBeenLastCalledWith(true);
            expect(audio.setNoiseSuppression).toHaveBeenCalledTimes(2);
        });

        it("does not report the initial state as a user choice", async () => {
            await renderWithDevices();
            act(() => handlers.onNoiseSuppression(false));
            await waitFor(() => expect(checkbox().checked).toBe(false));
            expect(audio.setNoiseSuppression).not.toHaveBeenCalled();
        });

        it("logs when the backend rejects the change", async () => {
            vi.mocked(audio.setNoiseSuppression).mockRejectedValue("no audio module");
            const user = userEvent.setup();
            await renderWithDevices();
            await user.click(checkbox());
            await waitFor(() => expect(console.error).toHaveBeenCalledWith(
                "Failed to set noise suppression:", "no audio module",
            ));
        });
    });
});
