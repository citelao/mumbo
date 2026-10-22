// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import LoginPage from "./LoginPage";
import * as audio from "../invoke/audio";

vi.mock("../invoke", async (orig) => ({
    ...(await orig<typeof import("../invoke")>()),
    getServerStats: vi.fn(async () => { throw new Error("offline"); }),
}));
vi.mock("../invoke/audio", async (orig) => ({
    ...(await orig<typeof import("../invoke/audio")>()),
    getAudioInputDevices: vi.fn(),
    getAudioOutputDevices: vi.fn(),
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

beforeEach(() => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(audio.getAudioInputDevices).mockResolvedValue(inputs);
    vi.mocked(audio.getAudioOutputDevices).mockResolvedValue(outputs);
});
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const select = (label: string) => screen.getByLabelText(label) as HTMLSelectElement;
const renderPage = () => render(<LoginPage onConnect={vi.fn()} onRemoveRecentServer={vi.fn()} />);

describe("LoginPage audio settings", () => {
    it("lists devices and labels the default", async () => {
        renderPage();
        await screen.findByRole("option", { name: /USB Mic/ });
        expect(screen.getByRole("option", { name: "USB Mic (Default)" })).toBeTruthy();
        expect(screen.getByRole("option", { name: "Built-in Mic" })).toBeTruthy();
        expect(screen.getByRole("option", { name: "Headphones" })).toBeTruthy();
    });

    it("preselects the default input and output", async () => {
        renderPage();
        await waitFor(() => expect(select("Audio input").value).toBe("in-b"));
        expect(select("Audio output").value).toBe("out-a");
    });

    it("changing the selection updates the select", async () => {
        const user = userEvent.setup();
        renderPage();
        await waitFor(() => expect(select("Audio input").value).toBe("in-b"));
        await user.selectOptions(select("Audio input"), "in-a");
        await user.selectOptions(select("Audio output"), "out-b");
        expect(select("Audio input").value).toBe("in-a");
        expect(select("Audio output").value).toBe("out-b");
    });

    it.todo("tells the backend when the user picks a device (setters are still todo!() in Rust)");

    it("handles no devices without crashing", async () => {
        vi.mocked(audio.getAudioInputDevices).mockResolvedValue([]);
        vi.mocked(audio.getAudioOutputDevices).mockResolvedValue([]);
        renderPage();
        await waitFor(() => expect(audio.getAudioOutputDevices).toHaveBeenCalled());
        expect(select("Audio input").options).toHaveLength(0);
        expect(select("Audio input").value).toBe("");
    });

    it("selects nothing when no device is flagged default", async () => {
        vi.mocked(audio.getAudioInputDevices).mockResolvedValue(
            inputs.map((d) => ({ ...d, is_default: false })),
        );
        renderPage();
        await screen.findByRole("option", { name: "USB Mic" });
        // A controlled <select> with value "" and no matching option shows the first option.
        expect(select("Audio input").selectedOptions[0]?.value).toBe("in-a");
    });

    it("shows empty selects and logs when device listing fails", async () => {
        vi.mocked(audio.getAudioInputDevices).mockRejectedValue("boom");
        renderPage();
        await waitFor(() => expect(console.error).toHaveBeenCalled());
        expect(select("Audio input").options).toHaveLength(0);
        expect(select("Audio output").options).toHaveLength(0);
        expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
    });
});
