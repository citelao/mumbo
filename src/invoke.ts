

import { Channel, invoke } from "@tauri-apps/api/core";
import { ServerToClientMessage } from "./protocol";
import z from "zod";

export interface GetServerStatsParams {
    server: string;
    port: number;
}

const ServerStatsSchema = z.object({
    server_version_v2: z.number(),
    user_count: z.number(),
    max_user_count: z.number(),
    max_bandwidth_per_user: z.number(),
});
export type ServerStats = z.infer<typeof ServerStatsSchema>;

export async function getServerStats(params: GetServerStatsParams): Promise<ServerStats> {
    const raw = await invoke("get_server_stats", { ...params });
    return ServerStatsSchema.parse(raw);
}

export interface ConnectParams {
    name: string;
    server: string;
    port: number;
    password: string;
    channel: Channel<ServerToClientMessage>;
}

export function connect(params: ConnectParams): Promise<void> {
    return invoke("connect", { ... params });
}

export function disconnect(): Promise<void> {
    return invoke("disconnect");
}

export function sendChannelMessage(message: string): Promise<void> {
    return invoke("send_channel_message", { message });
}

export function setSelfMute(muted: boolean): Promise<void> {
    return invoke("set_self_mute", { muted });
}
