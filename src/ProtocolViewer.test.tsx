// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ServerToClientMessage } from "./protocol";
import { ProtocolViewer, type ProtocolLogEntry } from "./ProtocolViewer";
// Real serde output; a Rust test asserts this file matches what Rust emits.
import fixture from "../crates/mumble-protocol/fixtures/server_to_client_messages.json";

afterEach(cleanup);

function fixtureMessage(label: string): ServerToClientMessage {
    const raw = fixture.find((message) => {
        const subtype = (message.data as { type?: unknown }).type;
        return message.type === "UdpTunnel" ? `UdpTunnel/${subtype}` === label : message.type === label;
    });
    if (!raw) {
        throw new Error(`No fixture message for ${label}`);
    }
    return ServerToClientMessage.parse(raw);
}

let nextId = 0;
function okEntry(label: string): ProtocolLogEntry {
    return { id: nextId++, receivedAt: new Date(), ok: true, message: fixtureMessage(label) };
}

function invalidEntry(): ProtocolLogEntry {
    return { id: nextId++, receivedAt: new Date(), ok: false, raw: { type: "Bogus" }, error: "bad message" };
}

function rowLabels(): string[] {
    // Each row's summary is "<time> <label>"; drop the time.
    return screen.getAllByRole("listitem").map((row) => {
        const summary = row.querySelector("summary")!;
        const time = summary.querySelector("time")!.textContent!;
        return summary.textContent!.slice(time.length).trim();
    });
}

const mixedEntries = [okEntry("Ping"), okEntry("UdpTunnel/Audio"), okEntry("UdpTunnel/Ping"), okEntry("UdpTunnel/Audio")];

describe("ProtocolViewer", () => {
    it("renders as a collapsed details element", () => {
        const { container } = render(<ProtocolViewer entries={[]} onClear={vi.fn()} />);
        const details = container.querySelector("details.protocol-viewer") as HTMLDetailsElement;
        expect(details).toBeTruthy();
        expect(details.open).toBe(false);
        expect(details.querySelector(":scope > summary")!.textContent).toBe("Protocol log (0)");
    });

    it("hides audio packets by default and counts only visible entries", () => {
        render(<ProtocolViewer entries={mixedEntries} onClear={vi.fn()} />);
        expect((screen.getByLabelText("Show audio packets") as HTMLInputElement).checked).toBe(false);
        expect(rowLabels()).toEqual(["Ping", "UdpTunnel/Ping"]);
        expect(screen.getByText("Protocol log (2)")).toBeTruthy();
    });

    it("shows audio packets once toggled on", async () => {
        const user = userEvent.setup();
        render(<ProtocolViewer entries={mixedEntries} onClear={vi.fn()} />);

        await user.click(screen.getByLabelText("Show audio packets"));

        expect(rowLabels()).toEqual(["Ping", "UdpTunnel/Audio", "UdpTunnel/Ping", "UdpTunnel/Audio"]);
        expect(screen.getByText("Protocol log (4)")).toBeTruthy();

        await user.click(screen.getByLabelText("Show audio packets"));
        expect(rowLabels()).toEqual(["Ping", "UdpTunnel/Ping"]);
    });

    it("labels invalid entries and shows their error", () => {
        render(<ProtocolViewer entries={[okEntry("Version"), invalidEntry()]} onClear={vi.fn()} />);

        expect(rowLabels()).toEqual(["Version", "Invalid message"]);
        const invalidRow = screen.getAllByRole("listitem")[1];
        expect(invalidRow.className).toBe("protocol-viewer-invalid");
        expect(invalidRow.querySelector("pre")!.textContent).toContain("bad message");
        expect(screen.getAllByRole("listitem")[0].className).toBe("");
    });

    it("calls onClear when Clear is clicked", async () => {
        const user = userEvent.setup();
        const onClear = vi.fn();
        render(<ProtocolViewer entries={mixedEntries} onClear={onClear} />);

        await user.click(screen.getByRole("button", { name: "Clear" }));

        expect(onClear).toHaveBeenCalledTimes(1);
    });
});
