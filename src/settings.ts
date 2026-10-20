import { load, Store } from '@tauri-apps/plugin-store';
import z from 'zod';

const SettingsSchema = z.object({
    name: z.string().optional(),
    server: z.string().optional(),
    port: z.number().optional(),
    password: z.string().optional(),
});

export type Settings = z.infer<typeof SettingsSchema>;

export default class SettingsStore {
    private store: Store;

    constructor(store: Store) {
        this.store = store;
    }

    static async create(): Promise<SettingsStore> {
        const store = await load("settings.json", {
            defaults: {} satisfies Settings,
            autoSave: false,
        });
        return new SettingsStore(store);
    }

    async getSettings(): Promise<Settings> {
        const raw = await this.store.get("settings");
        if (!raw) {
            return {};
        }

        const parsed = SettingsSchema.safeParse(raw);
        if (!parsed.success) {
            console.error("Failed to parse settings:", parsed.error);
            return {};
        }
        return parsed.data;
    }

    async saveSettings(settings: Settings): Promise<void> {
        await this.store.set("settings", settings);
        await this.store.save();
    }
}