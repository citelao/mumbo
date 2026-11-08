import { CheckMenuItem, Menu, PredefinedMenuItem, Submenu } from '@tauri-apps/api/menu';
import SettingsStore, { Theme } from './settings';
import { assertNever } from './typeHelpers';

export type OnThemeSelected = (theme: Theme) => void;

export type Platform = 'macos' | 'windows' | 'linux';

// Good enough to pick a menu layout; avoids pulling in the OS plugin.
export function currentPlatform(): Platform {
    const ua = navigator.userAgent;
    if (ua.includes('Mac')) return 'macos';
    if (ua.includes('Windows')) return 'windows';
    return 'linux';
}

export async function fetchWindowSubmenu(menu: Menu): Promise<Submenu> {
    const TAURI_WINDOW_SUBMENU_ID = "__tauri_window_menu__";
    const windowSubmenu = await menu.get(TAURI_WINDOW_SUBMENU_ID);
    if (!windowSubmenu || !(windowSubmenu instanceof Submenu)) {
        throw new Error(`Window submenu not found (${typeof windowSubmenu})`);
    }
    return windowSubmenu;
}

export async function fetchThemeMenuItem(menu: Menu, theme: Theme): Promise<CheckMenuItem> {
    const windowSubmenu = await fetchWindowSubmenu(menu);

    const themeSubmenu = await windowSubmenu.get('theme-submenu');
    if (!themeSubmenu || !(themeSubmenu instanceof Submenu)) {
        throw new Error(`Theme submenu not found (${typeof themeSubmenu})`);
    }

    const item = await themeSubmenu.get(`theme-${theme}`);
    if (!item || !(item instanceof CheckMenuItem)) {
        throw new Error(`Theme menu item not found (${typeof item})`);
    }
    return item;
}

const THEMES: { theme: Theme; text: string }[] = [
    { theme: 'system', text: 'System' },
    { theme: 'light', text: 'Light' },
    { theme: 'dark', text: 'Dark' },
];

/**
 * Builds the app menu from the saved settings, saving each change the user
 * makes through it.
 */
export async function createAppMenu(
    settings: SettingsStore,
    onThemeSelected: OnThemeSelected,
    platform: Platform = currentPlatform(),
): Promise<Menu> {
    const menu = await Menu.default();
    const initial = await settings.getSettings();
    const initialTheme = initial.theme ?? 'system';

    const themeSubmenu = await Submenu.new({
        text: 'Theme',
        id: 'theme-submenu',
        items: await Promise.all(THEMES.map(({ theme, text }) =>
            CheckMenuItem.new({
                id: `theme-${theme}`,
                text,
                checked: theme === initialTheme,
                action: async () => {
                    await settings.setTheme(theme);
                    // TODO: respond to the actual event.
                    for (const { theme: t } of THEMES) {
                        const item = await fetchThemeMenuItem(menu, t);
                        await item.setChecked(t === theme);
                    }
                    onThemeSelected(theme);
                },
            }),
        )),
    });

    // https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/crates/tauri/src/menu/menu.rs#L19
    const windowSubmenu = await fetchWindowSubmenu(menu);
    windowSubmenu.append(themeSubmenu);

    await addAutoUpdateMenuItem(menu, platform, initial.autoUpdate ?? true, async (enabled) => {
        await settings.setAutoUpdate(enabled);
    });

    // const chatSubmenu = await Submenu.new({
    //     text: 'Chat',
    //     items: [
    //         {
    //             id: 'new_chat',
    //             text: 'New Chat',
    //             action: () => {
    //                 console.log('New Chat clicked');
    //             },
    //         },
    //     ],
    // });
    // menu.insert(chatSubmenu, 1);

    return menu;
}

/**
 * Adds an "Update Automatically" toggle where each platform puts app-wide
 * options: the app menu on macOS, the Edit menu on Windows. Linux builds don't
 * self-update (their package managers do), so they get no item.
 */
async function addAutoUpdateMenuItem(
    menu: Menu,
    platform: Platform,
    checked: boolean,
    onToggled: (enabled: boolean) => Promise<void>,
): Promise<void> {
    if (platform === 'linux') {
        return;
    }

    const item: CheckMenuItem = await CheckMenuItem.new({
        id: 'auto-update',
        text: 'Update Automatically',
        checked,
        // Native check items toggle themselves on click; just report the result.
        action: async () => onToggled(await item.isChecked()),
    });

    // Tauri's default menu has no IDs on these submenus, so find them by
    // position/label: https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/crates/tauri/src/menu/menu.rs#L19
    const submenus = (await menu.items()).filter(i => i instanceof Submenu);
    switch (platform) {
        case 'macos': {
            // Right after "About mumbo", as macOS apps do for update options.
            const appSubmenu = submenus[0];
            if (!appSubmenu) {
                throw new Error('App submenu not found');
            }
            await appSubmenu.insert(item, 1);
            break;
        }
        case 'windows': {
            let editSubmenu: Submenu | undefined;
            for (const submenu of submenus) {
                if (await submenu.text() === 'Edit') {
                    editSubmenu = submenu;
                }
            }
            if (!editSubmenu) {
                throw new Error('Edit submenu not found');
            }
            await editSubmenu.append([await PredefinedMenuItem.new({ item: 'Separator' }), item]);
            break;
        }
        default:
            assertNever(platform);
    }
}
