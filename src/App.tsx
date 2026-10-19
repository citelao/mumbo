import { useEffect, useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { listen } from '@tauri-apps/api/event';
import { z } from "zod";
import { ServerToClientMessage } from "./protocol";
import { ProtocolViewer, type ProtocolLogEntry } from "./ProtocolViewer";
import { fetchUsers, type User } from "./mumbleState";
import "./App.css";
import LoginPage, { OnConnectParams, OnConnectResult } from "./pages/LoginPage";

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

function App() {
    const [protocolLog, setProtocolLog] = useState<ProtocolLogEntry[]>([]);

    const [isConnected, setIsConnected] = useState(false);

    const onConnect = async (params: OnConnectParams): Promise<OnConnectResult> => {
        // A channel can't be reused: when the Rust side drops it (the
        // command ends), Tauri unregisters its JS callback.
        const channel = new Channel<ServerToClientMessage>();
        channel.onmessage = (event) => {
            const entry = toLogEntry(event);
            setProtocolLog((log) => [...log, entry].slice(-MAX_LOG_ENTRIES));
        };
        try {
            // Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
            await invoke("connect", { ...params, channel });
        } catch (error) {
            console.error(error);
            alert("Failed to connect: " + error);
            return { succeeded: false, error: String(error) };
        }

        setIsConnected(true);
        return { succeeded: true };
    };

    const onDisconnect = async () => {
        try {
            await invoke("disconnect");
            setIsConnected(false);
        } catch (error) {
            console.error(error);
            alert("Failed to disconnect: " + error);
        }
    };

    if (!isConnected) {
        return <LoginPage onConnect={onConnect} />;
    } else {
        return <MainPage protocolLog={protocolLog} onClearProtocolLog={() => setProtocolLog([])} onDisconnect={onDisconnect} />;
    }
}

interface MainPageProps {
    protocolLog: ProtocolLogEntry[];
    onClearProtocolLog: () => void;

    onDisconnect: () => void;
}

function MainPage(props: MainPageProps) {
    const [users, setUsers] = useState<User[]>([]);

    useEffect(() => {
        let stopped = false;
        const refresh = async () => {
            const users = await fetchUsers();
            if (!stopped) {
                setUsers(users);
            }
        };

        console.log("Fetching initial state...");
        refresh();

        const unlisten = listen<unknown>("session-changed", (event) => {
            console.log("Session changed event received:", event);

            // TODO: parse the event.
            refresh();
        });
        return () => {
            stopped = true;
            unlisten.then(stop => stop());
        };
    }, []);

    return (
        <>
            <main className="container">
                <section className="user-list">
                    User list
                    <ul>
                        {users.map((user) => (
                            <li key={user.session}>
                            {user.name} (channel {user.channel_id})
                            {user.self_mute || user.mute ? " muted" : ""}
                            {user.self_deaf || user.deaf ? " deafened" : ""}
                            </li>
                        ))}
                    </ul>
                </section>
                <section className="chat">
                    Chat
                </section>

                <button type="button" onClick={props.onDisconnect}>
                    Disconnect
                </button>
            </main>
            <ProtocolViewer entries={props.protocolLog} onClear={props.onClearProtocolLog} />
        </>
    );
}

export default App;
