// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Channel, invoke } from "@tauri-apps/api/core";
import App from "./App";
import type SettingsStore from "./settings";
import users from "../crates/mumble-protocol/fixtures/users.json";
import channels from "../crates/mumble-protocol/fixtures/channels.json";
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
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

/** Answers the commands the app makes, like a server with nobody else on it. */
function fakeBackend(overrides: Partial<Record<string, unknown>> = {}) {
    const responses: Record<string, unknown> = { get_users: [], get_channels: [], get_chat_messages: [], ...overrides };
    mockInvoke.mockImplementation(async (command) => responses[command]);
}

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

/** In-memory stand-in; the real store needs the Tauri runtime. */
function fakeSettings(): SettingsStore {
    return {
        getSettings: async () => ({}),
        saveSettings: async () => {},
        addRecentServer: async () => {},
    } as unknown as SettingsStore;
}

function renderApp() {
    return render(<App settings={fakeSettings()} />);
}

describe("App", () => {
    it("shows the login page initially", () => {
        renderApp();
        expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Disconnect" })).toBeNull();
    });

    it("connects with the login settings and switches to the main page", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();

        await connect(user);

        expect(mockInvoke).toHaveBeenCalledWith("connect", {
            server: "localhost",
            port: 64738,
            password: "",
            name: "testo",
            channel: expect.any(Channel),
        });
        expect(screen.queryByRole("button", { name: "Connect" })).toBeNull();
    });

    it("stays on the login page and shows the error when connecting fails", async () => {
        const user = userEvent.setup();
        mockInvoke.mockRejectedValue("connection refused");
        renderApp();

        await user.click(screen.getByRole("button", { name: "Connect" }));

        expect(await screen.findByText("connection refused")).toBeTruthy();
        expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Disconnect" })).toBeNull();
        expect(window.alert).toHaveBeenCalledWith("Failed to connect: connection refused");
    });

    it("logs messages from the connection, flagging ones that fail the schema", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
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

    it("shows the server's users and chat once connected", async () => {
        const user = userEvent.setup();
        fakeBackend({
            get_users: users,
            get_channels: channels,
            get_chat_messages: [{ type: "ServerTextMessage", timestamp: "2026-10-19T05:31:31Z", content: "Welcome to the server!" }],
        });
        renderApp();

        await connect(user);

        expect(await screen.findByText(/Alice/)).toBeTruthy();
        expect(screen.getByText(/muted/)).toBeTruthy();
        expect(screen.getByText(/Welcome to the server!/)).toBeTruthy();
    });

    it("asks Rust to move us to the channel that was double-clicked", async () => {
        const user = userEvent.setup();
        fakeBackend({ get_users: users, get_channels: channels });
        renderApp();
        await connect(user);

        await user.dblClick(await screen.findByText("Lobby"));

        expect(mockInvoke).toHaveBeenCalledWith("set_channel", { channelId: 3 });
    });

    it("tells the user when changing channel fails", async () => {
        const user = userEvent.setup();
        fakeBackend({ get_users: users, get_channels: channels });
        const answer = mockInvoke.getMockImplementation()!;
        mockInvoke.mockImplementation(async (command, args) => {
            if (command === "set_channel") throw "No active connection";
            return answer(command, args);
        });
        renderApp();
        await connect(user);

        await user.dblClick(await screen.findByText("Lobby"));

        await vi.waitFor(() => expect(window.alert).toHaveBeenCalledWith("Failed to set channel: No active connection"));
    });

    it("asks Rust to send a direct message to the user that was double-clicked", async () => {
        const user = userEvent.setup();
        fakeBackend({ get_users: users, get_channels: channels });
        renderApp();
        await connect(user);

        await user.dblClick(await screen.findByText(/Alice/));

        expect(mockInvoke).toHaveBeenCalledWith("send_user_message", { message: "Hello Alice!", session: [7] });
    });

    it("tells the user when sending a direct message fails", async () => {
        const user = userEvent.setup();
        fakeBackend({ get_users: users, get_channels: channels });
        const answer = mockInvoke.getMockImplementation()!;
        mockInvoke.mockImplementation(async (command, args) => {
            if (command === "send_user_message") throw "No active connection";
            return answer(command, args);
        });
        renderApp();
        await connect(user);

        await user.dblClick(await screen.findByText(/Alice/));

        await vi.waitFor(() => expect(window.alert).toHaveBeenCalledWith("Failed to send user message: No active connection"));
        expect(console.error).toHaveBeenCalledWith("No active connection");
    });

    it("returns to the login page after disconnecting", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);

        await user.click(screen.getByRole("button", { name: "Disconnect" }));

        expect(await screen.findByRole("button", { name: "Connect" })).toBeTruthy();
        expect(mockInvoke).toHaveBeenCalledWith("disconnect");
    });
});
