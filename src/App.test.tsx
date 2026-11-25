// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Editor } from "@tiptap/core";
import App from "./App";
import type SettingsStore from "./settings";
import sdkUsers from "../fixtures/mumbo-sdk/users.json";
import channels from "../fixtures/mumbo-sdk/channels.json";
import messages from "../fixtures/mumbo-sdk/server_to_client_messages.json";

// Rust's get_users adds app-only settings to the SDK's users.
const users = sdkUsers.map((user) => ({ ...user, volume_adjustment_db: null }));

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
const mockListen = vi.mocked(listen);

/** Delivers a Tauri event from Rust to whatever is currently listening for it. */
function emitFromRust(event: string, payload: unknown) {
    const handlers = mockListen.mock.calls.filter(([name]) => name === event).map(([, handler]) => handler);
    expect(handlers).not.toEqual([]);
    act(() => handlers[handlers.length - 1]({ event, id: 0, payload }));
}

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
    mockListen.mockClear();
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

/** The chat editor's editable element. */
async function chatEditor(): Promise<HTMLElement> {
    return vi.waitFor(() => {
        const editor = document.querySelector<HTMLElement>('[contenteditable="true"]');
        if (editor === null) throw new Error("no chat editor yet");
        return editor;
    });
}

/** Pastes into `element` as the OS clipboard would, with `files` and/or `text`. */
function paste(element: HTMLElement, { files = [], text = "" }: { files?: File[]; text?: string }) {
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", {
        value: {
            getData: (type: string) => (type === "text/plain" ? text : ""),
            types: [...(files.length ? ["Files"] : []), ...(text ? ["text/plain"] : [])],
            files,
        },
    });
    element.dispatchEvent(event);
}

/** Drops `files` on `element`, as dragging them in from the OS would. */
function drop(element: Element, files: File[]): Event {
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", { value: { types: ["Files"], files, getData: () => "" } });
    element.dispatchEvent(event);
    return event;
}

function pngFile() {
    return new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "image.png", { type: "image/png" });
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

    it("keeps non-audio messages logged through a long stretch of audio", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);

        const channel = connectChannel();
        const version = messages.find((message) => message.type === "Version")!;
        const audio = messages.find((message) => message.type === "UdpTunnel" && (message.data as { type?: unknown }).type === "Audio")!;
        act(() => {
            channel.onmessage(version);
            for (let i = 0; i < 1000; i++) {
                channel.onmessage(audio);
            }
        });

        expect(screen.getByText("Protocol log (1)")).toBeTruthy();
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

    it("asks Rust to send a pasted image to the channel", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);

        paste(await chatEditor(), { files: [pngFile()] });

        await vi.waitFor(() =>
            expect(mockInvoke).toHaveBeenCalledWith("send_channel_image", {
                image: expect.stringMatching(/^data:image\/png;base64,/),
            }),
        );
    });

    it("tells the user when sending an image fails", async () => {
        const user = userEvent.setup();
        fakeBackend();
        const answer = mockInvoke.getMockImplementation()!;
        mockInvoke.mockImplementation(async (command, args) => {
            if (command === "send_channel_image") throw "This image is too large to send";
            return answer(command, args);
        });
        renderApp();
        await connect(user);

        paste(await chatEditor(), { files: [pngFile()] });

        await vi.waitFor(() =>
            expect(window.alert).toHaveBeenCalledWith("Failed to send image: This image is too large to send"),
        );
    });

    // Otherwise the webview opens the dropped file in place of the app.
    it("doesn't navigate to files dropped on the login page", () => {
        renderApp();
        const dragOver = new Event("dragover", { bubbles: true, cancelable: true });
        screen.getByRole("button", { name: "Connect" }).dispatchEvent(dragOver);
        expect(dragOver.defaultPrevented).toBe(true);
        expect(drop(screen.getByRole("button", { name: "Connect" }), [pngFile()]).defaultPrevented).toBe(true);
    });

    it("asks Rust to send an image dropped anywhere on the main page", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);

        const event = drop(screen.getByRole("button", { name: "Disconnect" }), [pngFile()]);

        expect(event.defaultPrevented).toBe(true);
        await vi.waitFor(() =>
            expect(mockInvoke).toHaveBeenCalledWith("send_channel_image", {
                image: expect.stringMatching(/^data:image\/png;base64,/),
            }),
        );
    });

    it("sends an image dropped on the chat editor only once", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);
        const editorElement = await chatEditor();
        // jsdom has no layout, so ProseMirror can't find a drop position (and
        // throws) unless we stub it. Tiptap hangs the editor off its element.
        const { editor } = editorElement as HTMLElement & { editor: Editor };
        editor.view.posAtCoords = () => ({ pos: 1, inside: -1 });

        drop(editorElement, [pngFile()]);

        await vi.waitFor(() => expect(mockInvoke).toHaveBeenCalledWith("send_channel_image", expect.anything()));
        // Give a second send, from the page as well as the editor, the chance to show up.
        await new Promise((resolve) => setTimeout(resolve, 20));
        expect(mockInvoke.mock.calls.filter(([command]) => command === "send_channel_image")).toHaveLength(1);
    });

    it("highlights the main page while a file is dragged over it", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);
        const page = document.querySelector("main")!;

        act(() => {
            const dragEnter = new Event("dragenter", { bubbles: true });
            Object.defineProperty(dragEnter, "dataTransfer", { value: { types: ["Files"] } });
            page.dispatchEvent(dragEnter);
        });
        expect(page.dataset.dropTarget).toBe("true");

        act(() => {
            drop(page, [pngFile()]);
        });
        expect(page.dataset.dropTarget).toBeUndefined();
    });

    it("asks Rust to send the typed message to the channel on Enter", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);

        const editor = await chatEditor();
        paste(editor, { text: "hello" });
        fireEvent.keyUp(editor, { key: "Enter" });

        expect(mockInvoke).toHaveBeenCalledWith("send_channel_message", { message: "hello" });
    });

    it("tells the user when sending a message fails", async () => {
        const user = userEvent.setup();
        fakeBackend();
        const answer = mockInvoke.getMockImplementation()!;
        mockInvoke.mockImplementation(async (command, args) => {
            if (command === "send_channel_message") throw "This message is too long to send";
            return answer(command, args);
        });
        renderApp();
        await connect(user);

        const editor = await chatEditor();
        paste(editor, { text: "hello" });
        fireEvent.keyUp(editor, { key: "Enter" });

        await vi.waitFor(() =>
            expect(window.alert).toHaveBeenCalledWith("Failed to send message: This message is too long to send"),
        );
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

    it("returns to the login page and says why when the server drops us", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);

        emitFromRust("disconnected", { reason: { kind: "removed", reason: "spam", by: "admin", banned: false } });

        expect(await screen.findByRole("button", { name: "Connect" })).toBeTruthy();
        expect(screen.getByRole("status").textContent).toBe("You were removed from the server by admin: spam");
        expect(window.alert).not.toHaveBeenCalled();
    });

    it("forgets why it was last disconnected once it connects again", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);
        emitFromRust("disconnected", { reason: { kind: "server_closed" } });
        await screen.findByRole("status");

        await connect(user);
        await user.click(screen.getByRole("button", { name: "Disconnect" }));

        await screen.findByRole("button", { name: "Connect" });
        expect(screen.queryByRole("status")).toBeNull();
    });

    it("stays connected when a disconnect event fails the schema", async () => {
        const user = userEvent.setup();
        fakeBackend();
        renderApp();
        await connect(user);

        emitFromRust("disconnected", { reason: { kind: "exploded" } });

        expect(screen.getByRole("button", { name: "Disconnect" })).toBeTruthy();
        expect(console.error).toHaveBeenCalled();
    });
});
