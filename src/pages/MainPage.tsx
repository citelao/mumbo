import { use, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { fetchChatMessages, fetchUsers, TextMessage, type User } from "../mumbleState";
import { ProtocolViewer, type ProtocolLogEntry } from "../ProtocolViewer";
import "./MainPage.scss";

export interface MainPageProps {
    protocolLog: ProtocolLogEntry[];
    onClearProtocolLog: () => void;

    onDisconnect: () => void;
    onSendMessage: (message: string) => void;
}

export function MainPage(props: MainPageProps) {
    const [users, setUsers] = useState<User[]>([]);
    const [chatMessages, setChatMessages] = useState<TextMessage[]>([]);

    useEffect(() => {
        let stopped = false;
        const refresh = async () => {
            try {
                const users = await fetchUsers();
                const chatMessages = await fetchChatMessages();
                if (!stopped) {
                    setUsers(users);
                    setChatMessages(chatMessages);
                }
            } catch (error) {
                // Keep showing the last good state; the next session change retries.
                console.error("Failed to refresh session state:", error);
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
                    Users
                    <ul>
                        {users.map((user) => (
                            <li key={user.session}>
                            {user.is_speaking ? "● " : "◯ "}
                            {user.name} (channel {user.channel_id})
                            {user.self_mute || user.mute ? " muted" : ""}
                            {user.self_deaf || user.deaf ? " deafened" : ""}
                            </li>
                        ))}
                    </ul>

                    <button type="button" onClick={props.onDisconnect}>
                        Disconnect
                    </button>
                </section>
                <section className="chat">
                    Chat

                    <ol>
                        {chatMessages.map((message, index) => {
                            if (message.type === "ServerTextMessage") {
                                return (
                                    <li key={index}>
                                        <span className="date">[{new Date(message.timestamp).toLocaleTimeString()}]</span>
                                        <span className="content" dangerouslySetInnerHTML={{ __html: message.content }}></span>
                                    </li>
                                );
                            }

                            const user = users.find(u => u.session === message.user_session);
                            return (
                                <li key={index}>
                                    <span className="date">[{new Date(message.timestamp).toLocaleTimeString()}]</span>
                                    <span className="user">{user ? user.name : "Unknown"}:</span>
                                    <span className="content" dangerouslySetInnerHTML={{ __html: message.content }}></span>
                                </li>
                            );
                        })}
                    </ol>
                    <form onSubmit={(e) => {
                        e.preventDefault();
                        const input = e.currentTarget.querySelector('input[type="text"]') as HTMLInputElement;
                        if (input && input.value.trim() !== "") {
                            props.onSendMessage(input.value.trim());
                            input.value = "";
                        }
                    }}>
                        <label>
                            Chat
                            <input type="text" placeholder="Message" onKeyUp={(e) => {
                                if (e.key === "Enter") {
                                    e.currentTarget.form?.dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
                                }
                            }} />
                        </label>
                    </form>
                </section>
            </main>
            <ProtocolViewer entries={props.protocolLog} onClear={props.onClearProtocolLog} />
        </>
    );
}