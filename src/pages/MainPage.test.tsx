// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { MainPage } from "./MainPage";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import users from "../../crates/mumble-protocol/fixtures/users.json";

// There's no Tauri runtime under vitest.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
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

function renderMainPage(onDisconnect = () => {}) {
    return render(<MainPage protocolLog={[]} onClearProtocolLog={() => {}} onDisconnect={onDisconnect} />);
}

describe("MainPage", () => {
    it("shows the users and chat already on the server", async () => {
        backend = { users, chat: ["Welcome to the server!"] };

        renderMainPage();

        expect(await screen.findByText(/Alice \(channel 3\) muted/)).toBeTruthy();
        expect(screen.getByText("Welcome to the server!")).toBeTruthy();
    });

    it("refreshes when the session changes", async () => {
        renderMainPage();
        await vi.waitFor(() => expect(sessionChanged).toBeDefined());
        expect(screen.queryByText(/Alice/)).toBeNull();

        backend = { users, chat: ["hi"] };
        act(() => sessionChanged!());

        expect(await screen.findByText(/Alice \(channel 3\)/)).toBeTruthy();
        expect(screen.getByText("hi")).toBeTruthy();
    });

    it("keeps chat messages in the order they were sent", async () => {
        backend.chat = ["first", "second", "third"];

        renderMainPage();

        await screen.findByText("third");
        const chat = screen.getAllByRole("list")[1];
        expect([...chat.querySelectorAll("li")].map((li) => li.textContent)).toEqual(["first", "second", "third"]);
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
});
