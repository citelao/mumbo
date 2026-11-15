import { describe, expect, it, vi } from "vitest";

const created = vi.hoisted(() => [] as { id: string; checked: boolean }[]);

// The real menu needs the Tauri runtime; record what gets created instead.
vi.mock("@tauri-apps/api/menu", () => {
    class Submenu {
        static async new() { return new Submenu(); }
        append = vi.fn();
    }
    class CheckMenuItem {
        static async new(opts: { id: string; checked: boolean }) {
            created.push(opts);
            return new CheckMenuItem();
        }
    }
    class Menu {
        static async default() {
            const windowSubmenu = new Submenu();
            const menu = new Menu();
            menu.get = async () => windowSubmenu;
            return menu;
        }
        get: () => Promise<unknown> = async () => null;
    }
    return { Menu, Submenu, CheckMenuItem };
});

import { createAppMenu } from "./appMenu";

function checkedThemes() {
    return Object.fromEntries(created.map(i => [i.id, i.checked]));
}

describe("createAppMenu", () => {
    it("checks only the initial theme", async () => {
        created.length = 0;
        await createAppMenu(() => {}, "dark");

        expect(checkedThemes()).toEqual({
            "theme-system": false,
            "theme-light": false,
            "theme-dark": true,
        });
    });

    it("checks System by default", async () => {
        created.length = 0;
        await createAppMenu(() => {});

        expect(checkedThemes()).toEqual({
            "theme-system": true,
            "theme-light": false,
            "theme-dark": false,
        });
    });
});
