import { load, Store } from '@tauri-apps/plugin-store';
import z from 'zod';

const ServerSchema = z.object({
    name: z.string().optional(),
    server: z.string().optional(),
    port: z.number().optional(),
    password: z.string().optional(),
});

const SettingsSchema = z.object({
    recentServers: z.array(ServerSchema).optional()
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

    async addRecentServer(server: { name?: string; server?: string; port?: number; password?: string }): Promise<void> {
        const settings = await this.getSettings();
        const recentServers = settings.recentServers ?? [];

        // Remove any existing entry that matches the new server
        //
        // Password is not considered when determining if a server already exists in the recent list.
        const index = recentServers.findIndex(s => s.server === server.server && s.port === server.port && s.name === server.name);
        if (index !== -1) {
            recentServers.splice(index, 1);
        }

        recentServers.unshift(server);
        settings.recentServers = recentServers.slice(0, 5); // Keep only the 5 most recent servers
        await this.saveSettings(settings);
    }
}