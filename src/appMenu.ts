import { CheckMenuItem, Menu, Submenu } from '@tauri-apps/api/menu';
import { Theme } from './settings';

export type OnThemeSelected = (theme: Theme) => void;

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

export async function createAppMenu(onThemeSelected: OnThemeSelected, initialTheme: Theme = 'system'): Promise<Menu> {
    const menu = await Menu.default();

    const themeSubmenu = await Submenu.new({
        text: 'Theme',
        id: 'theme-submenu',
        items: await Promise.all(THEMES.map(({ theme, text }) =>
            CheckMenuItem.new({
                id: `theme-${theme}`,
                text,
                checked: theme === initialTheme,
                action: () => {
                    onThemeSelected(theme);
                },
            }),
        )),
    });

    // https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/crates/tauri/src/menu/menu.rs#L19
    const windowSubmenu = await fetchWindowSubmenu(menu);
    windowSubmenu.append(themeSubmenu);

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