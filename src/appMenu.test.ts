import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Store } from "@tauri-apps/plugin-store";
import SettingsStore, { Settings } from "./settings";

type FakeCheckItem = { id: string; checked: boolean; action?: () => unknown };
const created = vi.hoisted(() => [] as FakeCheckItem[]);

// The real menu needs the Tauri runtime; record what gets created instead.
vi.mock("@tauri-apps/api/menu", () => {
    class CheckMenuItem {
        checked: boolean;
        action?: () => unknown;
        constructor(public opts: { id: string; checked: boolean; action?: () => unknown }) {
            this.checked = opts.checked;
            this.action = opts.action;
        }
        get id() { return this.opts.id; }
        static async new(opts: { id: string; checked: boolean }) {
            const item = new CheckMenuItem(opts);
            created.push(item);
            return item;
        }
        async isChecked() { return this.checked; }
        async setChecked(checked: boolean) { this.checked = checked; }
    }
    class Submenu {
        constructor(public label = "", public children: unknown[] = []) {}
        static async new(opts: { items?: unknown[] }) { return new Submenu("", opts.items ?? []); }
        async text() { return this.label; }
        async get(id: string) {
            return this.children.find(c => c instanceof CheckMenuItem && c.opts.id === id)
                ?? this.children.find(c => c instanceof Submenu);
        }
        append = vi.fn(async (items: unknown) => { this.children.push(...[items].flat()); });
        insert = vi.fn();
    }
    class PredefinedMenuItem {
        static async new() { return new PredefinedMenuItem(); }
    }
    class Menu {
        submenus = [new Submenu("mumbo"), new Submenu("Edit"), new Submenu("Window")];
        static async default() { return new Menu(); }
        async get() { return this.submenus[2]; }
        async items() { return this.submenus; }
    }
    return { Menu, Submenu, CheckMenuItem, PredefinedMenuItem };
});

import { createAppMenu } from "./appMenu";

type FakeSubmenu = { append: ReturnType<typeof vi.fn>; insert: ReturnType<typeof vi.fn> };
type FakeMenu = { submenus: FakeSubmenu[] };

// The real store needs the Tauri runtime, so back it with a plain map.
function fakeSettings(settings: Settings = {}) {
    const data = new Map<string, unknown>([["settings", settings]]);
    const store = {
        get: async (key: string) => data.get(key),
        set: async (key: string, value: unknown) => { data.set(key, value); },
        save: async () => {},
    };
    return { data, settings: new SettingsStore(store as unknown as Store) };
}

function item(id: string) {
    const found = created.find(i => i.id === id);
    if (!found) {
        throw new Error(`No menu item ${id}`);
    }
    return found;
}

function checkedThemes() {
    return Object.fromEntries(created.filter(i => i.id.startsWith("theme-")).map(i => [i.id, i.checked]));
}

beforeEach(() => {
    created.length = 0;
});

describe("createAppMenu theme", () => {
    it("checks only the saved theme", async () => {
        await createAppMenu(fakeSettings({ theme: "dark" }).settings, () => {}, "macos");

        expect(checkedThemes()).toEqual({
            "theme-system": false,
            "theme-light": false,
            "theme-dark": true,
        });
    });

    it("moves the check to the chosen theme", async () => {
        await createAppMenu(fakeSettings({ theme: "dark" }).settings, () => {}, "macos");

        await item("theme-light").action?.();

        expect(checkedThemes()).toEqual({
            "theme-system": false,
            "theme-light": true,
            "theme-dark": false,
        });
    });

    it("checks System by default", async () => {
        await createAppMenu(fakeSettings().settings, () => {}, "macos");

        expect(checkedThemes()).toEqual({
            "theme-system": true,
            "theme-light": false,
            "theme-dark": false,
        });
    });

    it("saves and reports the chosen theme", async () => {
        const { settings, data } = fakeSettings();
        const onThemeSelected = vi.fn();
        await createAppMenu(settings, onThemeSelected, "macos");

        await item("theme-light").action?.();

        expect(data.get("settings")).toEqual({ theme: "light" });
        expect(onThemeSelected).toHaveBeenCalledWith("light");
    });
});

describe("createAppMenu auto-update", () => {
    it("goes right after About in the app menu on macOS", async () => {
        const menu = await createAppMenu(fakeSettings().settings, () => {}, "macos") as unknown as FakeMenu;

        const [app, edit] = menu.submenus;
        expect(app.insert).toHaveBeenCalledWith(expect.objectContaining({ checked: true }), 1);
        expect(edit.append).not.toHaveBeenCalled();
    });

    it("goes at the end of the Edit menu on Windows", async () => {
        const menu = await createAppMenu(fakeSettings().settings, () => {}, "windows") as unknown as FakeMenu;

        const [app, edit] = menu.submenus;
        expect(edit.append).toHaveBeenCalledOnce();
        expect(app.insert).not.toHaveBeenCalled();
    });

    it("is left out on Linux, where package managers handle updates", async () => {
        const menu = await createAppMenu(fakeSettings().settings, () => {}, "linux") as unknown as FakeMenu;

        expect(created.map(i => i.id)).not.toContain("auto-update");
        const [app, edit] = menu.submenus;
        expect(app.insert).not.toHaveBeenCalled();
        expect(edit.append).not.toHaveBeenCalled();
    });

    it("reflects the saved setting", async () => {
        await createAppMenu(fakeSettings({ autoUpdate: false }).settings, () => {}, "macos");

        expect(item("auto-update").checked).toBe(false);
    });

    it("saves the item's new checked state when clicked", async () => {
        const { settings, data } = fakeSettings({ autoUpdate: false, theme: "dark" });
        await createAppMenu(settings, () => {}, "macos");

        // Native check items flip themselves before the action runs.
        const autoUpdate = item("auto-update");
        autoUpdate.checked = true;
        await autoUpdate.action?.();

        expect(data.get("settings")).toEqual({ autoUpdate: true, theme: "dark" });
    });
});
