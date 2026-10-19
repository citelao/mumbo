// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Channel, invoke } from "@tauri-apps/api/core";
import App from "./App";
import users from "../crates/mumble-protocol/fixtures/users.json";
import messages from "../crates/mumble-protocol/fixtures/server_to_client_messages.json";

// There's no Tauri runtime under vitest. The fake Channel only needs to hold
// the `onmessage` handler so tests can push messages through it.
vi.mock("@tauri-apps/api/core", () => ({
    invoke: vi.fn(),
    Channel: class {
        onmessage: (message: unknown) => void = () => {};
    },
}));
const mockInvoke = vi.mocked(invoke);

function connectChannel(): Channel<unknown> {
    const call = mockInvoke.mock.calls.find(([command]) => command === "connect");
    return (call![1] as { channel: Channel<unknown> }).channel;
}

beforeEach(() => {
    mockInvoke.mockReset();
    vi.spyOn(window, "alert").mockImplementation(() => {});
    vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

async function connect(user: ReturnType<typeof userEvent.setup>) {
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByRole("button", { name: "Disconnect" });
}

describe("App", () => {
    it("shows the login page initially", () => {
        render(<App />);
        expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Disconnect" })).toBeNull();
    });

    it("connects with the login settings and switches to the main page", async () => {
        const user = userEvent.setup();
        mockInvoke.mockResolvedValue(undefined);
        render(<App />);

        await connect(user);

        expect(mockInvoke).toHaveBeenCalledWith("connect", {
            server: "warmitup.chat",
            port: 64738,
            password: "",
            name: "besto_testo",
            channel: expect.any(Channel),
        });
        expect(screen.queryByRole("button", { name: "Connect" })).toBeNull();
    });

    it("stays on the login page and shows the error when connecting fails", async () => {
        const user = userEvent.setup();
        mockInvoke.mockRejectedValue("connection refused");
        render(<App />);

        await user.click(screen.getByRole("button", { name: "Connect" }));

        expect(await screen.findByText("connection refused")).toBeTruthy();
        expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Disconnect" })).toBeNull();
        expect(window.alert).toHaveBeenCalledWith("Failed to connect: connection refused");
    });

    it("logs messages from the connection, flagging ones that fail the schema", async () => {
        const user = userEvent.setup();
        mockInvoke.mockResolvedValue(undefined);
        render(<App />);
        await connect(user);

        const channel = connectChannel();
        const version = messages.find((message) => message.type === "Version")!;
        act(() => {
            channel.onmessage(version);
            channel.onmessage({ type: "Version", data: { unexpected: true } });
        });

        expect(screen.getByText("Protocol log (2)")).toBeTruthy();
        const rows = screen.getAllByRole("listitem");
        expect(rows[0].className).toBe("");
        expect(rows[1].className).toBe("protocol-viewer-invalid");
    });

    it("shows users after refreshing", async () => {
        const user = userEvent.setup();
        mockInvoke.mockImplementation(async (command) => (command === "get_users" ? users : undefined));
        render(<App />);
        await connect(user);

        await user.click(screen.getByRole("button", { name: "Refresh users" }));

        expect(await screen.findByText(/Alice \(channel 3\)/)).toBeTruthy();
        expect(screen.getByText(/muted/)).toBeTruthy();
    });

    it("returns to the login page after disconnecting", async () => {
        const user = userEvent.setup();
        mockInvoke.mockResolvedValue(undefined);
        render(<App />);
        await connect(user);

        await user.click(screen.getByRole("button", { name: "Disconnect" }));

        expect(await screen.findByRole("button", { name: "Connect" })).toBeTruthy();
        expect(mockInvoke).toHaveBeenCalledWith("disconnect");
    });
});
