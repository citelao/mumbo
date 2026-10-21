import { useEffect, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { z } from "zod";
import { ServerToClientMessage } from "./protocol";
import type { ProtocolLogEntry } from "./ProtocolViewer";
import "./App.css";
import LoginPage, { OnConnectParams, OnConnectResult } from "./pages/LoginPage";
import { MainPage } from "./pages/MainPage";
import SettingsStore, { Settings } from "./settings";
import { connect, disconnect, sendChannelMessage } from "./invoke";

// Keeps the log (and React re-renders) bounded during long sessions.
const MAX_LOG_ENTRIES = 500;

let nextLogEntryId = 0;

function toLogEntry(event: ServerToClientMessage): ProtocolLogEntry {
    // The static type is only a promise; re-parse to catch drift between
    // the Rust enum and this schema.
    const result = ServerToClientMessage.safeParse(event);
    const base = { id: nextLogEntryId++, receivedAt: new Date() };
    if (!result.success) {
        const error = z.prettifyError(result.error);
        console.error("Message failed schema validation:", error, event);
        return { ...base, ok: false, raw: event, error };
    }
    return { ...base, ok: true, message: result.data };
}

interface AppProps {
    settings: SettingsStore;
}

function App(props: AppProps) {
    const [protocolLog, setProtocolLog] = useState<ProtocolLogEntry[]>([]);

    const [isConnected, setIsConnected] = useState(false);

    const [settings, setSettings] = useState<Settings | null>(null);
    useEffect(() => {
        (async () => {
            const s = await props.settings.getSettings();
            setSettings(s);
        })();
    }, [props.settings]);

    const onConnect = async (params: OnConnectParams): Promise<OnConnectResult> => {
        // A channel can't be reused: when the Rust side drops it (the
        // command ends), Tauri unregisters its JS callback.
        const channel = new Channel<ServerToClientMessage>();
        channel.onmessage = (event) => {
            const entry = toLogEntry(event);
            // TODO: don't exceed the maximum number of log entries for audio messages.
            // if (entry.ok && entry.message.type === "UdpTunnel" && entry.message.data.type === "Audio") {
            //     return;
            // }

            setProtocolLog((log) => [...log, entry].slice(-MAX_LOG_ENTRIES));
        };
        try {
            // Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
            await connect({ ...params, channel });
        } catch (error) {
            console.error(error);
            alert("Failed to connect: " + error);
            return { succeeded: false, error: String(error) };
        }

        setIsConnected(true);

        // If connected successfully, update the recent servers in the settings.
        await props.settings.addRecentServer({ name: params.name, server: params.server, port: params.port, password: params.password });

        return { succeeded: true };
    };

    const onDisconnect = async () => {
        try {
            await disconnect();
            setIsConnected(false);
        } catch (error) {
            console.error(error);
            alert("Failed to disconnect: " + error);
        }
    };

    const mostRecentServer = settings?.recentServers?.[0];

    if (!isConnected) {
        return <LoginPage
            recentServers={settings?.recentServers}
            name={mostRecentServer?.name}
            server={mostRecentServer?.server}
            port={mostRecentServer?.port}
            password={mostRecentServer?.password}
            onConnect={onConnect} />;
    } else {
        return <MainPage
            protocolLog={protocolLog}
            onClearProtocolLog={() => setProtocolLog([])}
            onSendMessage={sendChannelMessage}
            onDisconnect={onDisconnect} />;
    }
}

export default App;
