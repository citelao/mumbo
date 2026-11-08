import { Menu, Submenu } from '@tauri-apps/api/menu';

export async function createAppMenu(): Promise<Menu> {
    // Will become the application submenu on MacOS
    const aboutSubmenu = await Submenu.new({
        text: 'About',
        items: [
            {
                id: 'quit',
                text: 'Quit',
                action: () => {
                    console.log('Quit pressed');
                },
            },
        ],
    });
    
    const fileSubmenu = await Submenu.new({
        text: 'File',
        icon: 'Folder', // Optional: Add an icon to the submenu
        items: [
            {
                id: 'new',
                text: 'New',
                action: () => {
                    console.log('New clicked');
                },
            },
            {
                id: 'open',
                text: 'Open',
                action: () => {
                    console.log('Open clicked');
                },
            },
            {
                id: 'save_as',
                text: 'Save As...',
                action: () => {
                    console.log('Save As clicked');
                },
            },
        ],
    });
    
    const editSubmenu = await Submenu.new({
        text: 'Edit',
        items: [
            {
                id: 'undo',
                text: 'Undo',
                action: () => {
                    console.log('Undo clicked');
                },
            },
            {
                id: 'redo',
                text: 'Redo',
                action: () => {
                    console.log('Redo clicked');
                },
            },
        ],
    });
    
    const menu = await Menu.new({
        items: [aboutSubmenu, fileSubmenu, editSubmenu],
    });
    aboutSubmenu.close();
    fileSubmenu.close();
    editSubmenu.close();
    return menu;
}