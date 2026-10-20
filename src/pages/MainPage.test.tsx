// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { MainPage } from "./MainPage";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import users from "../../crates/mumble-protocol/fixtures/users.json";

// There's no Tauri runtime under vitest.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

/** What the fake Rust side currently holds. */
let backend: { users: unknown[]; chat: unknown[] };
let sessionChanged: (() => void) | undefined;
let unlisten: Mock<() => void>;

beforeEach(() => {
    backend = { users: [], chat: [] };
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
        switch (command) {
            case "get_users": return backend.users;
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

function renderMainPage(onDisconnect = () => {}) {
    return render(<MainPage protocolLog={[]} onClearProtocolLog={() => {}} onDisconnect={onDisconnect} />);
}

describe("MainPage", () => {
    it("shows the users and chat already on the server", async () => {
        backend = {
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

        backend = { users, chat: [chat("hi")] };
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

    it("keeps chat messages in the order they were sent", async () => {
        backend.chat = [chat("first"), chat("second"), chat("third")];

        renderMainPage();

        await screen.findByText(/third$/);
        const list = screen.getAllByRole("list")[1];
        expect([...list.querySelectorAll("li")].map((li) => li.textContent)).toEqual([
            expect.stringMatching(/first$/),
            expect.stringMatching(/second$/),
            expect.stringMatching(/third$/),
        ]);
    });

    it("shows who sent each chat message", async () => {
        const alice = users.find((u) => u.name === "Alice")!;
        backend = { users, chat: [chat("hello", alice.session), chat("anyone?")] };

        renderMainPage();

        expect(await screen.findByText(/Alice: hello$/)).toBeTruthy();
        expect(screen.getByText(/Unknown: anyone\?$/)).toBeTruthy();
    });

    it("stops listening when it goes away", async () => {
        const { unmount } = renderMainPage();
        await vi.waitFor(() => expect(mockListen).toHaveBeenCalled());

        unmount();

        await vi.waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
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
