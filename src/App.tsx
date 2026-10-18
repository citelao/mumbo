import { useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { z } from "zod";
import { ServerToClientMessage } from "./protocol";
import { ProtocolViewer, type ProtocolLogEntry } from "./ProtocolViewer";
import { fetchUsers, type User } from "./mumbleState";
import "./App.css";

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
    const [name, setName] = useState("besto_testo");
    const [server, setServer] = useState("warmitup.chat");
    const [port, setPort] = useState(64738);
    const [password, setPassword] = useState("");
    const [protocolLog, setProtocolLog] = useState<ProtocolLogEntry[]>([]);
    const [users, setUsers] = useState<User[]>([]);
    
    return (
        <>
        <main className="container">
        <section className="user-list">
        User list
        <button
        type="button"
        onClick={async () => {
            try {
                setUsers(await fetchUsers());
            } catch (error) {
                console.error("Failed to fetch users:", error);
            }
        }}
        >
        Refresh users
        </button>
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
        
        {/* Form for connection */}
        <form className="connection-form"
        onSubmit={async (e) => {
            e.preventDefault();
            // A channel can't be reused: when the Rust side drops it (the
            // command ends), Tauri unregisters its JS callback.
            const channel = new Channel<ServerToClientMessage>();
            channel.onmessage = (event) => {
                const entry = toLogEntry(event);
                setProtocolLog((log) => [...log, entry].slice(-MAX_LOG_ENTRIES));
            };
            try {
                // Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
                await invoke("connect_and_listen", { server, port, password, name, channel });
            } catch (error) {
                console.error(error);
                alert("Failed to connect: " + error);
            }
        }}>
        <label>
        Server
        <input placeholder="Server" value={server} onChange={(e) => setServer(e.target.value)} />
        </label>
        <label>
        Port
        <input placeholder="Port" value={port} onChange={(e) => setPort(Number(e.target.value))} />
        </label>
        <label>
        Password
        <input placeholder="Password" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </label>
        <label>
        Name
        <input placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <br />
        <button type="submit">
        Connect/Disconnect
        </button>
        </form>
        <button type="button" onClick={async () => {
            try {
                await invoke("disconnect");
            } catch (error) {
                console.error(error);
                alert("Failed to disconnect: " + error);
            }
        }}>
        Disconnect
        </button>
        </main>
        <ProtocolViewer entries={protocolLog} onClear={() => setProtocolLog([])} />
        </>
    );
}

export default App;
