import { Menu, Submenu } from '@tauri-apps/api/menu';

export async function createAppMenu(): Promise<Menu> {
    const menu = await Menu.default();

    const themeSubmenu = await Submenu.new({
        text: 'Theme',
        items: [
            {
                id: 'system',
                text: 'System',
                action: () => {
                    console.log('System theme selected');
                },
            },
            {
                id: 'light',
                text: 'Light',
                action: () => {
                    console.log('Light theme selected');
                },
            },
            {
                id: 'dark',
                text: 'Dark',
                action: () => {
                    console.log('Dark theme selected');
                },
            },
        ],
    });

    // https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/crates/tauri/src/menu/menu.rs#L19
    const TAURI_WINDOW_SUBMENU_ID = "__tauri_window_menu__";
    const windowSubmenu = await menu.get(TAURI_WINDOW_SUBMENU_ID);
    if (!windowSubmenu || !(windowSubmenu instanceof Submenu)) {
        throw new Error(`Window submenu not found (${typeof windowSubmenu})`);
    }
    windowSubmenu.append(themeSubmenu);

    const chatSubmenu = await Submenu.new({
        text: 'Chat',
        items: [
            {
                id: 'new_chat',
                text: 'New Chat',
                action: () => {
                    console.log('New Chat clicked');
                },
            },
        ],
    });
    menu.insert(chatSubmenu, 1);

    return menu;
}