import { useEffect, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { z } from "zod";
import { ServerToClientMessage } from "./protocol";
import { appendToProtocolLog, emptyProtocolLog, type ProtocolLog, type ProtocolLogEntry } from "./ProtocolViewer";
import "./App.scss";
import LoginPage, { OnConnectParams, OnConnectResult } from "./pages/LoginPage";
import { MainPage } from "./pages/MainPage";
import SettingsStore, { Server, Settings } from "./settings";
import { connect, disconnect, sendChannelMessage, sendChannelImage, setChannel, setSelfMute, sendUserMessage } from "./invoke";
import type { Channel as MumbleChannel } from "./invoke/mumbleState";
import { onDisconnected, type DisconnectReason } from "./invoke/connection";

// Keeps each log feed (and React re-renders) bounded during long sessions.
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
    const [protocolLog, setProtocolLog] = useState<ProtocolLog>(emptyProtocolLog);

    const [isConnected, setIsConnected] = useState(false);
    // If the last connection ended on its own, this is set.
    const [disconnectReason, setDisconnectReason] = useState<DisconnectReason | null>(null);

    // Rust says when the connection ends on its own (e.g. the server went
    // away); disconnecting ourselves goes through `onDisconnect` instead.
    useEffect(() => {
        if (!isConnected) {
            return;
        }
        const unlisten = onDisconnected((reason) => {
            setIsConnected(false);
            setDisconnectReason(reason);
        });
        return () => {
            unlisten.then((stop) => stop());
        };
    }, [isConnected]);

    // With Tauri's own drag and drop off (so the chat editor gets HTML drop
    // events), the webview opens any file dropped outside a drop target in
    // place of the app.
    useEffect(() => {
        const preventNavigation = (event: DragEvent) => event.preventDefault();
        window.addEventListener("dragover", preventNavigation);
        window.addEventListener("drop", preventNavigation);
        return () => {
            window.removeEventListener("dragover", preventNavigation);
            window.removeEventListener("drop", preventNavigation);
        };
    }, []);

    const [settings, setSettings] = useState<Settings | null>(null);
    useEffect(() => {
        (async () => {
            const s = await props.settings.getSettings();
            setSettings(s);
        })();
    }, [props.settings]);

    const onRemoveRecentServer = async (server: Server) => {
        await props.settings.removeRecentServer(server);
        setSettings(await props.settings.getSettings());
    };
    const onSendMessage = async (message: string) => {
        try {
            await sendChannelMessage(message);
        } catch (error) {
            console.error(error);
            alert("Failed to send message: " + error);
        }
    };
    const onSendImage = async (image: string) => {
        try {
            await sendChannelImage(image);
        } catch (error) {
            console.error(error);
            alert("Failed to send image: " + error);
        }
    };
    const onSendUserMessage = async (message: string, session: number[]) => {
        try {
            await sendUserMessage(message, session);
        } catch (error) {
            console.error(error);
            alert("Failed to send user message: " + error);
        }
    };

    const onConnect = async (params: OnConnectParams): Promise<OnConnectResult> => {
        // A channel can't be reused: when the Rust side drops it (the
        // command ends), Tauri unregisters its JS callback.
        const channel = new Channel<ServerToClientMessage>();
        channel.onmessage = (event) => {
            const entry = toLogEntry(event);
            setProtocolLog((log) => appendToProtocolLog(log, entry, MAX_LOG_ENTRIES));
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
        setDisconnectReason(null);

        // If connected successfully, update the recent servers in the settings.
        await props.settings.addRecentServer({ name: params.name, server: params.server, port: params.port, password: params.password });
        setSettings(await props.settings.getSettings());

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
    const onSetChannel = async (channel: MumbleChannel) => {
        try {
            await setChannel(channel);
        } catch (error) {
            console.error(error);
            alert("Failed to set channel: " + error);
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
            disconnectReason={disconnectReason ?? undefined}
            onConnect={onConnect}
            onRemoveRecentServer={onRemoveRecentServer} />;
    } else {
        return <MainPage
            protocolLog={protocolLog}
            onClearProtocolLog={() => setProtocolLog(emptyProtocolLog)}
            onSendMessage={onSendMessage}
            onSendImage={onSendImage}
            onSetSelfMute={setSelfMute}
            onDisconnect={onDisconnect}
            onSetChannel={onSetChannel}
            onSendUserMessage={onSendUserMessage} />;
    }
}

export default App;
