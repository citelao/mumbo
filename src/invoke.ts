import { Channel, invoke } from "@tauri-apps/api/core";
import { ServerToClientMessage } from "./protocol";

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

export function sendMessage(message: string): Promise<void> {
    return invoke("send_message", { message });
}