import { describe, expect, it, vi } from "vitest";
import type { Store } from "@tauri-apps/plugin-store";
import SettingsStore from "./settings";

// The real store needs the Tauri runtime, so back it with a plain map.
function fakeStore(initial: Record<string, unknown> = {}) {
    const data = new Map(Object.entries(initial));
    const store = {
        get: vi.fn(async (key: string) => data.get(key)),
        set: vi.fn(async (key: string, value: unknown) => { data.set(key, value); }),
        save: vi.fn(async () => {}),
    };
    return { store, data, settings: new SettingsStore(store as unknown as Store) };
}

const a = { name: "A", server: "a.example.com", port: 64738, password: "pw-a" };
const b = { name: "B", server: "b.example.com", port: 64738 };
const c = { name: "C", server: "c.example.com", port: 1234 };

describe("SettingsStore.removeRecentServer", () => {
    it("removes only the matching server and keeps the order of the rest", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a, b, c] } });

        await settings.removeRecentServer(b);

        expect((await settings.getSettings()).recentServers).toEqual([a, c]);
    });

    it("matches ignoring the password", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a, b] } });

        await settings.removeRecentServer({ name: a.name, server: a.server, port: a.port });

        expect((await settings.getSettings()).recentServers).toEqual([b]);
    });

    it("does not remove servers that differ in name, host or port", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a] } });

        await settings.removeRecentServer({ ...a, name: "other" });
        await settings.removeRecentServer({ ...a, server: "other.example.com" });
        await settings.removeRecentServer({ ...a, port: 1 });

        expect((await settings.getSettings()).recentServers).toEqual([a]);
    });

    it("can empty the list", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a] } });

        await settings.removeRecentServer(a);

        expect((await settings.getSettings()).recentServers).toEqual([]);
    });

    it("persists the change to disk", async () => {
        const { store, data, settings } = fakeStore({ settings: { recentServers: [a, b] } });

        await settings.removeRecentServer(a);

        expect(data.get("settings")).toEqual({ recentServers: [b] });
        expect(store.save).toHaveBeenCalledTimes(1);
    });

    it("is a no-op, without writing, when there are no recent servers", async () => {
        const { store, settings } = fakeStore();

        await settings.removeRecentServer(a);

        expect(store.set).not.toHaveBeenCalled();
        expect(await settings.getSettings()).toEqual({});
    });

    it("leaves the list unchanged when the server isn't in it", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a, b] } });

        await settings.removeRecentServer(c);

        expect((await settings.getSettings()).recentServers).toEqual([a, b]);
    });

    it("lets a removed server be re-added as most recent", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a, b] } });

        await settings.removeRecentServer(a);
        await settings.addRecentServer(a);

        expect((await settings.getSettings()).recentServers).toEqual([a, b]);
    });
});

describe("SettingsStore.addRecentServer", () => {
    it("saves the first server when nothing is stored", async () => {
        const { store, data, settings } = fakeStore();

        await settings.addRecentServer(a);

        expect(data.get("settings")).toEqual({ recentServers: [a] });
        expect(store.save).toHaveBeenCalledTimes(1);
    });

    it("puts the newest server first", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a] } });

        await settings.addRecentServer(b);

        expect((await settings.getSettings()).recentServers).toEqual([b, a]);
    });

    it("moves an existing server to the front instead of duplicating it", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a, b, c] } });

        await settings.addRecentServer(c);

        expect((await settings.getSettings()).recentServers).toEqual([c, a, b]);
    });

    it("treats a server with a different password as the same one, keeping the new password", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a, b] } });

        await settings.addRecentServer({ ...a, password: "new-pw" });

        expect((await settings.getSettings()).recentServers).toEqual([{ ...a, password: "new-pw" }, b]);
    });

    it("treats servers differing in name, host or port as distinct", async () => {
        const { settings } = fakeStore({ settings: { recentServers: [a] } });

        await settings.addRecentServer({ ...a, name: "other" });
        await settings.addRecentServer({ ...a, server: "other.example.com" });
        await settings.addRecentServer({ ...a, port: 1 });

        expect((await settings.getSettings()).recentServers).toHaveLength(4);
    });

    it("keeps only the 5 most recent servers", async () => {
        const servers = Array.from({ length: 6 }, (_, i) => ({ name: `S${i}`, server: `s${i}.example.com`, port: 64738 }));
        const { settings } = fakeStore();

        for (const s of servers) {
            await settings.addRecentServer(s);
        }

        // The first one added is the one dropped.
        expect((await settings.getSettings()).recentServers).toEqual(servers.slice(1).reverse());
    });

    it("doesn't drop an existing server when re-adding at the cap", async () => {
        const servers = Array.from({ length: 5 }, (_, i) => ({ name: `S${i}`, server: `s${i}.example.com`, port: 64738 }));
        const { settings } = fakeStore({ settings: { recentServers: servers } });

        await settings.addRecentServer(servers[4]);

        expect((await settings.getSettings()).recentServers).toHaveLength(5);
    });
});

describe("SettingsStore.getSettings", () => {
    it("returns empty settings when nothing is stored", async () => {
        const { settings } = fakeStore();

        expect(await settings.getSettings()).toEqual({});
    });

    it("returns empty settings when the stored data is invalid", async () => {
        vi.spyOn(console, "error").mockImplementation(() => {});
        const { settings } = fakeStore({ settings: { recentServers: [{ port: "not-a-number" }] } });

        expect(await settings.getSettings()).toEqual({});
    });
});
