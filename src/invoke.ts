

import { Channel, invoke } from "@tauri-apps/api/core";
import { ServerToClientMessage } from "./protocol";
import z from "zod";

export interface GetServerStatsParams {
    server: string;
    port: number;
}

const AudioDeviceSchema = z.object({
    id: z.string(),
    name: z.string(),
    is_default: z.boolean(),
});
export type AudioDevice = z.infer<typeof AudioDeviceSchema>;
export async function getAudioInputDevices(): Promise<AudioDevice[]> {
    const raw = await invoke("get_audio_input_devices");
    return z.array(AudioDeviceSchema).parse(raw);
}
export async function setAudioInputDevice(deviceId: string): Promise<void> {
    return invoke("set_audio_input_device", { deviceId });
}
export async function getAudioOutputDevices(): Promise<AudioDevice[]> {
    const raw = await invoke("get_audio_output_devices");
    return z.array(AudioDeviceSchema).parse(raw);
}
export async function setAudioOutputDevice(deviceId: string): Promise<void> {
    return invoke("set_audio_output_device", { deviceId });
}

const ServerStatsSchema = z.object({
    round_trip_time: z.number(),
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
