// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import LoginPage, { type OnConnectResult } from "./LoginPage";

afterEach(cleanup);

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((r) => {
        resolve = r;
    });
    return { promise, resolve };
}

function input(label: string): HTMLInputElement {
    return screen.getByLabelText(label) as HTMLInputElement;
}

describe("LoginPage", () => {
    it.each([
        [
            "an admin banning us",
            { kind: "removed", reason: "spam", by: "admin", banned: true } as const,
            "You were banned from the server by admin: spam",
        ],
        [
            "the server removing us",
            { kind: "removed", reason: "You connected to the server from another device", by: null, banned: false } as const,
            "You were removed from the server: You connected to the server from another device",
        ],
        [
            "a removal with no reason",
            { kind: "removed", reason: null, by: null, banned: false } as const,
            "You were removed from the server.",
        ],
        ["a clean close", { kind: "server_closed" } as const, "The server closed the connection."],
    ])("explains %s", (_, disconnectReason, message) => {
        render(<LoginPage onConnect={vi.fn()} onRemoveRecentServer={vi.fn()} disconnectReason={disconnectReason} />);
        expect(screen.getByRole("status").textContent).toBe(message);
    });

    it("explains a lost connection, with what went wrong", () => {
        render(
            <LoginPage
                onConnect={vi.fn()}
                onRemoveRecentServer={vi.fn()}
                disconnectReason={{ kind: "connection_lost", detail: "IO error: peer closed connection" }} />,
        );
        const status = screen.getByRole("status");
        expect(status.textContent).toContain("Lost the connection to the server.");
        expect(status.textContent).toContain("IO error: peer closed connection");
    });

    it("says nothing about disconnecting when we weren't disconnected", () => {
        render(<LoginPage onConnect={vi.fn()} onRemoveRecentServer={vi.fn()} />);
        expect(screen.queryByRole("status")).toBeNull();
    });

    it("renders the default connection settings", () => {
        render(<LoginPage onConnect={vi.fn()} onRemoveRecentServer={vi.fn()} />);
        expect(input("Server").value).toBe("localhost");
        expect(input("Port").value).toBe("64738");
        expect(input("Password").value).toBe("");
        expect(input("Name").value).toBe("testo");
        expect(screen.getByRole("button", { name: "Connect" })).toHaveProperty("disabled", false);
    });

    it("submits the entered settings, with the port as a number", async () => {
        const user = userEvent.setup();
        const onConnect = vi.fn(async (): Promise<OnConnectResult> => ({ succeeded: true }));
        render(<LoginPage onConnect={onConnect} onRemoveRecentServer={vi.fn()} />);

        await user.clear(input("Server"));
        await user.type(input("Server"), "example.com");
        await user.clear(input("Port"));
        await user.type(input("Port"), "1234");
        await user.type(input("Password"), "hunter2");
        await user.clear(input("Name"));
        await user.type(input("Name"), "alice");
        await user.click(screen.getByRole("button", { name: "Connect" }));

        expect(onConnect).toHaveBeenCalledTimes(1);
        expect(onConnect).toHaveBeenCalledWith({
            server: "example.com",
            port: 1234,
            password: "hunter2",
            name: "alice",
        });
    });

    it("disables the button and shows progress while connecting", async () => {
        const user = userEvent.setup();
        const pending = deferred<OnConnectResult>();
        render(<LoginPage onConnect={() => pending.promise} onRemoveRecentServer={vi.fn()} />);

        await user.click(screen.getByRole("button", { name: "Connect" }));
        const button = screen.getByRole("button", { name: "Connecting..." });
        expect(button).toHaveProperty("disabled", true);

        pending.resolve({ succeeded: true });
        const restored = await screen.findByRole("button", { name: "Connect" });
        expect(restored).toHaveProperty("disabled", false);
    });

    it("shows the error when connecting fails", async () => {
        const user = userEvent.setup();
        render(<LoginPage onConnect={async () => ({ succeeded: false, error: "connection refused" })} onRemoveRecentServer={vi.fn()} />);

        await user.click(screen.getByRole("button", { name: "Connect" }));

        expect(await screen.findByText("connection refused")).toBeTruthy();
    });

    it("clears a previous error when connecting succeeds", async () => {
        const user = userEvent.setup();
        const onConnect = vi
            .fn<() => Promise<OnConnectResult>>()
            .mockResolvedValueOnce({ succeeded: false, error: "connection refused" })
            .mockResolvedValueOnce({ succeeded: true });
        render(<LoginPage onConnect={onConnect} onRemoveRecentServer={vi.fn()} />);

        await user.click(screen.getByRole("button", { name: "Connect" }));
        await screen.findByText("connection refused");
        await user.click(screen.getByRole("button", { name: "Connect" }));

        await screen.findByRole("button", { name: "Connect" });
        expect(onConnect).toHaveBeenCalledTimes(2);
        expect(screen.queryByText("connection refused")).toBeNull();
    });
});
