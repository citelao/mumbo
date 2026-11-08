import { beforeEach, describe, expect, it, vi } from "vitest";

const created = vi.hoisted(() => [] as { id: string; checked: boolean; action?: () => unknown }[]);

// The real menu needs the Tauri runtime; record what gets created instead.
vi.mock("@tauri-apps/api/menu", () => {
    class Submenu {
        constructor(public label = "") {}
        static async new() { return new Submenu(); }
        async text() { return this.label; }
        append = vi.fn();
        insert = vi.fn();
    }
    class CheckMenuItem {
        checked: boolean;
        constructor(opts: { checked: boolean }) { this.checked = opts.checked; }
        static async new(opts: { id: string; checked: boolean }) {
            created.push(opts);
            return new CheckMenuItem(opts);
        }
        async isChecked() { return this.checked; }
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

import { Menu } from "@tauri-apps/api/menu";
import { addAutoUpdateMenuItem, createAppMenu } from "./appMenu";

beforeEach(() => {
    created.length = 0;
});

function checkedThemes() {
    return Object.fromEntries(created.map(i => [i.id, i.checked]));
}

describe("createAppMenu", () => {
    it("checks only the initial theme", async () => {
        await createAppMenu(() => {}, "dark");

        expect(checkedThemes()).toEqual({
            "theme-system": false,
            "theme-light": false,
            "theme-dark": true,
        });
    });

    it("checks System by default", async () => {
        await createAppMenu(() => {});

        expect(checkedThemes()).toEqual({
            "theme-system": true,
            "theme-light": false,
            "theme-dark": false,
        });
    });
});

type FakeMenu = Menu & { submenus: { label: string; append: ReturnType<typeof vi.fn>; insert: ReturnType<typeof vi.fn> }[] };

describe("addAutoUpdateMenuItem", () => {
    it("goes right after About in the app menu on macOS", async () => {
        const menu = await Menu.default() as FakeMenu;

        await addAutoUpdateMenuItem(menu, "macos", true, () => {});

        const [app, edit] = menu.submenus;
        expect(app.insert).toHaveBeenCalledWith(expect.anything(), 1);
        expect(edit.append).not.toHaveBeenCalled();
    });

    it("goes at the end of the Edit menu on Windows", async () => {
        const menu = await Menu.default() as FakeMenu;

        await addAutoUpdateMenuItem(menu, "windows", true, () => {});

        const [app, edit] = menu.submenus;
        expect(edit.append).toHaveBeenCalledOnce();
        expect(app.insert).not.toHaveBeenCalled();
    });

    it("is left out on Linux, where package managers handle updates", async () => {
        const menu = await Menu.default() as FakeMenu;

        await addAutoUpdateMenuItem(menu, "linux", true, () => {});

        expect(created).toEqual([]);
        for (const submenu of menu.submenus) {
            expect(submenu.append).not.toHaveBeenCalled();
            expect(submenu.insert).not.toHaveBeenCalled();
        }
    });

    it("reflects the saved setting", async () => {
        await addAutoUpdateMenuItem(await Menu.default(), "macos", false, () => {});

        expect(created).toEqual([expect.objectContaining({ id: "auto-update", checked: false })]);
    });

    it("reports the item's new checked state when clicked", async () => {
        const onToggled = vi.fn();
        await addAutoUpdateMenuItem(await Menu.default(), "macos", false, onToggled);

        // Native check items flip themselves before the action runs.
        await created[0].action?.();

        expect(onToggled).toHaveBeenCalledWith(false);
    });
});
