// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { MainPage } from "./MainPage";
import type { Channel } from "../invoke/mumbleState";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import users from "../../crates/mumble-protocol/fixtures/users.json";
import channels from "../../crates/mumble-protocol/fixtures/channels.json";

// There's no Tauri runtime under vitest.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
// Capture the context menu instead of popping a native one.
const menu = vi.hoisted(() => ({ popup: vi.fn(), items: [] as { id: string; text: string; action: () => void }[] }));
vi.mock("@tauri-apps/api/menu", () => ({
    Menu: { new: vi.fn(async (options: { items: typeof menu.items }) => { menu.items = options.items; return { popup: menu.popup }; }) },
    MenuItem: { new: vi.fn(async (options: unknown) => options) },
}));
const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

/** What the fake Rust side currently holds. */
let backend: { users: unknown[]; channels: unknown[]; chat: unknown[] };
let sessionChanged: (() => void) | undefined;
let unlisten: Mock<() => void>;

beforeEach(() => {
    backend = { users: [], channels, chat: [] };
    menu.popup.mockReset();
    menu.items = [];
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
        switch (command) {
            case "get_users": return backend.users;
            case "get_channels": return backend.channels;
            case "get_chat_messages": return backend.chat;
            default: throw new Error(`unexpected command ${command}`);
        }
    });
    unlisten = vi.fn<() => void>();
    mockListen.mockReset();
    mockListen.mockImplementation(async (event, handler) => {
        expect(event).toBe("session-changed");
        sessionChanged = () => handler({ event, id: 0, payload: "UsersChanged" });
        return unlisten;
    });
});

afterEach(() => {
    cleanup();
    sessionChanged = undefined;
});

/** A chat message as Rust serializes it. */
function chat(content: string, user_session: number | null = null) {
    return { type: "ChatTextMessage", timestamp: "2026-10-19T05:31:31Z", user_session, content };
}

const alice = users[0];

/** Each line of the user list. */
function userLines(): string[] {
    const list = screen.getAllByRole("list")[0];
    // Users are nested under their channel's item, so skip the channels.
    return [...list.querySelectorAll("li")].map((li) => li.textContent ?? "").filter((text) => /^[●◯]/.test(text));
}

/** Matches a whole chat line, whose message content is rendered in its own element. */
function chatLine(pattern: RegExp) {
    return (_: string, element: Element | null) =>
        element?.tagName === "LI" && pattern.test(element.textContent ?? "");
}

function renderMainPage(
    onDisconnect = () => {},
    onSetSelfMute: (muted: boolean) => void = () => {},
    onSetChannel: (channel: Channel) => void = () => {},
) {
    return render(<MainPage protocolLog={[]} onClearProtocolLog={() => {}} onSendMessage={async () => {}} onSetSelfMute={onSetSelfMute} onDisconnect={onDisconnect} onSetChannel={onSetChannel} />);
}

describe("MainPage", () => {
    it("shows the users and chat already on the server", async () => {
        backend = {
            ...backend,
            users,
            chat: [{ type: "ServerTextMessage", timestamp: "2026-10-19T05:31:31Z", content: "Welcome to the server!" }],
        };

        renderMainPage();

        expect(await screen.findByText(/Alice \(channel 3\) muted/)).toBeTruthy();
        expect(screen.getByText(/Welcome to the server!/)).toBeTruthy();
    });

    it("refreshes when the session changes", async () => {
        renderMainPage();
        await vi.waitFor(() => expect(sessionChanged).toBeDefined());
        expect(screen.queryByText(/Alice/)).toBeNull();

        backend = { ...backend, users, chat: [chat("hi")] };
        act(() => sessionChanged!());

        expect(await screen.findByText(/Alice \(channel 3\)/)).toBeTruthy();
        expect(screen.getByText(/hi$/)).toBeTruthy();
    });

    it("logs a failed refresh and recovers on the next change", async () => {
        const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
        backend.users = [{ unexpected: true }];   // fails the User schema

        renderMainPage();

        await vi.waitFor(() => expect(consoleError).toHaveBeenCalled());
        backend.users = users;
        act(() => sessionChanged!());
        expect(await screen.findByText(/Alice \(channel 3\)/)).toBeTruthy();
        consoleError.mockRestore();
    });

    it("marks only the users who are speaking", async () => {
        backend.users = [
            { ...alice, is_speaking: true },
            { ...alice, session: 8, name: "Bob", is_speaking: false },
        ];

        renderMainPage();

        await screen.findByText(/Bob/);
        expect(userLines()).toEqual([
            expect.stringMatching(/^● Alice/),
            expect.stringMatching(/^◯ Bob/),
        ]);
    });

    it("shows when someone starts and stops speaking", async () => {
        backend.users = [alice];
        renderMainPage();
        expect(await screen.findByText(/◯ Alice/)).toBeTruthy();

        backend.users = [{ ...alice, is_speaking: true }];
        act(() => sessionChanged!());
        expect(await screen.findByText(/● Alice/)).toBeTruthy();

        backend.users = [alice];
        act(() => sessionChanged!());
        expect(await screen.findByText(/◯ Alice/)).toBeTruthy();
    });

    it("keeps chat messages in the order they were sent", async () => {
        backend.chat = [chat("first"), chat("second"), chat("third")];

        renderMainPage();

        await screen.findByText(/third$/);
        const list = screen.getAllByRole("list").find((l) => l.tagName === "OL")!;
        expect([...list.querySelectorAll("li")].map((li) => li.textContent)).toEqual([
            expect.stringMatching(/first$/),
            expect.stringMatching(/second$/),
            expect.stringMatching(/third$/),
        ]);
    });

    it("shows who sent each chat message", async () => {
        const alice = users.find((u) => u.name === "Alice")!;
        backend = { ...backend, users, chat: [chat("hello", alice.session), chat("anyone?")] };

        renderMainPage();

        expect(await screen.findByText(chatLine(/Alice:\s*hello$/))).toBeTruthy();
        expect(screen.getByText(chatLine(/Unknown:\s*anyone\?$/))).toBeTruthy();
    });

    it("stops listening when it goes away", async () => {
        const { unmount } = renderMainPage();
        await vi.waitFor(() => expect(mockListen).toHaveBeenCalled());

        unmount();

        await vi.waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
    });

    it("toggles self mute", async () => {
        const onSetSelfMute = vi.fn();
        renderMainPage(() => {}, onSetSelfMute);

        await userEvent.click(screen.getByRole("button", { name: "Mute" }));
        expect(onSetSelfMute).toHaveBeenLastCalledWith(true);

        await userEvent.click(screen.getByRole("button", { name: "Unmute" }));
        expect(onSetSelfMute).toHaveBeenLastCalledWith(false);
    });

    it("disconnects when asked", async () => {
        const user = userEvent.setup();
        const onDisconnect = vi.fn();
        renderMainPage(onDisconnect);

        await user.click(screen.getByRole("button", { name: "Disconnect" }));

        expect(onDisconnect).toHaveBeenCalledOnce();
    });

    it("opens chat links in the system browser instead of navigating", async () => {
        backend.chat = [chat('<a href="https://example.com/x">link</a>')];
        renderMainPage();
        const link = await screen.findByRole("link", { name: "link" });
        expect(link.getAttribute("target")).toBe("_blank");
        expect(link.getAttribute("rel")).toBe("noopener noreferrer");

        const notPrevented = fireEvent.click(link);

        expect(notPrevented).toBe(false);
        expect(openUrl).toHaveBeenCalledWith("https://example.com/x");
    });

    it("never opens non-web chat links", async () => {
        backend.chat = [chat('<a href="javascript:alert(1)">link</a>')];
        renderMainPage();
        const link = await screen.findByText("link");
        // DOMPurify drops disallowed schemes, leaving an inert anchor.
        expect(link.getAttribute("href")).toBeNull();

        fireEvent.click(link);

        expect(openUrl).not.toHaveBeenCalled();
    });
});

describe("MainPage channels", () => {
    const [root, lobby] = channels;
    const bob = { ...alice, session: 8, name: "Bob", channel_id: 0 };

    /** The list item for a channel, found by its heading text. */
    async function channelItem(name: string) {
        return (await screen.findByText(new RegExp(`^${name} \\(parent`))).closest("li")!;
    }

    it("shows each user under their own channel", async () => {
        backend.users = [alice, bob];

        renderMainPage();

        const inLobby = await channelItem("Lobby");
        expect(inLobby.textContent).toContain("Alice");
        expect(inLobby.textContent).not.toContain("Bob");
        const inRoot = await channelItem("Root");
        expect(inRoot.textContent).toContain("Bob");
        expect(inRoot.textContent).not.toContain("Alice");
    });

    it("shows where each channel sits in the tree", async () => {
        renderMainPage();

        expect(await screen.findByText("Root (parent none)")).toBeTruthy();
        expect(screen.getByText("Lobby (parent 0)")).toBeTruthy();
    });

    it("joins a channel on double click", async () => {
        const user = userEvent.setup();
        const onSetChannel = vi.fn();
        renderMainPage(() => {}, () => {}, onSetChannel);

        await user.dblClick(await screen.findByText("Lobby (parent 0)"));

        expect(onSetChannel).toHaveBeenCalledExactlyOnceWith(lobby);
    });

    it("doesn't join a channel on a single click", async () => {
        const user = userEvent.setup();
        const onSetChannel = vi.fn();
        renderMainPage(() => {}, () => {}, onSetChannel);

        await user.click(await screen.findByText("Lobby (parent 0)"));

        expect(onSetChannel).not.toHaveBeenCalled();
    });

    it("offers to join a channel from its context menu", async () => {
        const onSetChannel = vi.fn();
        renderMainPage(() => {}, () => {}, onSetChannel);
        const label = await screen.findByText("Root (parent none)");

        fireEvent.contextMenu(label, { clientX: 12, clientY: 34 });

        await vi.waitFor(() => expect(menu.popup).toHaveBeenCalledOnce());
        expect(menu.items.map((item) => item.text)).toEqual(["Join Root"]);
        expect(menu.popup).toHaveBeenCalledWith(expect.objectContaining({ x: 12, y: 34 }));
        expect(onSetChannel).not.toHaveBeenCalled();

        menu.items[0].action();

        expect(onSetChannel).toHaveBeenCalledExactlyOnceWith(root);
    });

    it("suppresses the browser's own context menu", async () => {
        renderMainPage();
        const label = await screen.findByText("Root (parent none)");

        const notPrevented = fireEvent.contextMenu(label);

        expect(notPrevented).toBe(false);
    });

    it("highlights the channel the current user is in", async () => {
        backend.users = [{ ...alice, is_me: true }, bob];

        renderMainPage();

        const mine = await screen.findByText("Lobby (parent 0)");
        const other = screen.getByText("Root (parent none)");
        expect(mine.className).not.toBe("");
        expect(other.className).toBe("");
    });

    it("highlights only the current user", async () => {
        backend.users = [{ ...alice, is_me: true }, bob];

        renderMainPage();

        await screen.findByText(/Bob/);
        const lines = [...document.querySelectorAll("li")].filter((li) => /^[●◯]/.test(li.textContent ?? ""));
        const highlighted = lines.filter((li) => li.className !== "");
        expect(highlighted).toHaveLength(1);
        expect(highlighted[0].textContent).toContain("Alice");
    });

    it("highlights no channel before we know who we are", async () => {
        backend.users = [alice];

        renderMainPage();

        await screen.findByText(/Alice/);
        expect(screen.getByText("Lobby (parent 0)").className).toBe("");
    });
});
