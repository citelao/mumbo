import { Menu } from '@tauri-apps/api/menu';

export async function createAppMenu(): Promise<Menu> {
    return await Menu.new({
        items: [
            {
                id: 'quit',
                text: 'Quit',
                action: () => {
                    console.log('quit pressed');
                },
            },
            {
                id: 'check_item',
                text: 'Check Item',
                checked: true,
            },
            {
                item: 'Separator',
            },
            {
                id: 'disabled_item',
                text: 'Disabled Item',
                enabled: false,
            },
            {
                id: 'status',
                text: 'Status: Processing...',
            },
        ]
    });
}