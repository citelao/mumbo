import { Menu, Submenu } from '@tauri-apps/api/menu';

export async function createAppMenu(): Promise<Menu> {
    const aboutSub = await Submenu.new({
        text: 'MyApp',
        items: [{ id: 'quit', text: 'Quit' }]
    });
    
    const fileSub = await Submenu.new({
        text: 'File',
        items: [{ id: 'open', text: 'Open' }]
    });
    
    const menu = await Menu.new({
        items: [aboutSub, fileSub]
    });
    aboutSub.close();
    fileSub.close();
    return menu;
}