import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { fetchChatMessages, fetchUsers, type User } from "../mumbleState";
import { ProtocolViewer, type ProtocolLogEntry } from "../ProtocolViewer";

export interface MainPageProps {
    protocolLog: ProtocolLogEntry[];
    onClearProtocolLog: () => void;

    onDisconnect: () => void;
}

export function MainPage(props: MainPageProps) {
    const [users, setUsers] = useState<User[]>([]);
    const [chatMessages, setChatMessages] = useState<string[]>([]);

    useEffect(() => {
        let stopped = false;
        const refresh = async () => {
            const users = await fetchUsers();
            const chatMessages = await fetchChatMessages();
            if (!stopped) {
                setUsers(users);
            }

            if (!stopped) {
                setChatMessages(chatMessages);
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

                    <ol>
                        {chatMessages.map((message, index) => (
                            <li key={index}>{message}</li>
                        ))}
                    </ol>
                    <label>
                        Chat
                        <textarea placeholder="Message" />
                    </label>
                </section>

                <button type="button" onClick={props.onDisconnect}>
                    Disconnect
                </button>
            </main>
            <ProtocolViewer entries={props.protocolLog} onClear={props.onClearProtocolLog} />
        </>
    );
}