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
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

/** Answers the commands the app makes, like a server with nobody else on it. */
function fakeBackend(overrides: Partial<Record<string, unknown>> = {}) {
    const responses: Record<string, unknown> = { get_users: [], get_chat_messages: [], ...overrides };
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

function renderApp() {
    return render(<App settings={{}} onSaveSettings={async () => {}} />);
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
            get_chat_messages: [{ type: "ServerTextMessage", timestamp: "2026-10-19T05:31:31Z", content: "Welcome to the server!" }],
        });
        renderApp();

        await connect(user);

        expect(await screen.findByText(/Alice \(channel 3\) muted/)).toBeTruthy();
        expect(screen.getByText(/Welcome to the server!/)).toBeTruthy();
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
