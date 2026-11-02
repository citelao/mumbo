import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Channel, fetchChannels, fetchChatMessages, fetchUsers, TextMessage, type User } from "../invoke/mumbleState";
import { ProtocolViewer, type ProtocolLogEntry } from "../ProtocolViewer";
import styles from "./MainPage.module.scss";
import type { Channel as MumbleChannel } from "../invoke/mumbleState";
import { Menu, MenuItem } from "@tauri-apps/api/menu";
import { LogicalPosition } from "@tauri-apps/api/dpi";

export interface MainPageProps {
    protocolLog: ProtocolLogEntry[];
    onClearProtocolLog: () => void;

    onDisconnect: () => void;
    onSendMessage: (message: string) => void;
    onSetSelfMute: (muted: boolean) => void;

    // Temporary
    onSetChannel: (channel: MumbleChannel) => void;
}

// Chat content is server-supplied HTML. Without this, clicking a link would
// navigate the whole webview away from the app instead of opening a browser.
function openChatLink(e: React.MouseEvent) {
    const href = (e.target as HTMLElement).closest("a")?.getAttribute("href");
    if (href === undefined || href === null) return;
    e.preventDefault();
    if (/^(https?:\/\/|mailto:)/i.test(href)) {
        void openUrl(href);
    }
}

export function MainPage(props: MainPageProps) {
    const [users, setUsers] = useState<User[]>([]);
    const [channels, setChannels] = useState<Channel[]>([]);
    const [chatMessages, setChatMessages] = useState<TextMessage[]>([]);
    const [selfMuted, setSelfMuted] = useState(false);

    useEffect(() => {
        let stopped = false;
        const refresh = async () => {
            try {
                const users = await fetchUsers();
                const channels = await fetchChannels();
                const chatMessages = await fetchChatMessages();
                if (!stopped) {
                    setUsers(users);
                    setChannels(channels);
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

    // onContextMenu={(e) => onUserContextMenu(user, e)}
    // const onUserContextMenu = async (user: User, event: React.MouseEvent) => {
    //     event.preventDefault();

    //     // TODO: real actions.
    //     const menu = await Menu.new({
    //         items: [
    //             await MenuItem.new({
    //                 id: "kick",
    //                 text: "Kick"
    //             })
    //         ]
    //     });
    //     void menu.popup(new LogicalPosition(event.clientX, event.clientY));
    // };

    const onSetChannel = (channel: MumbleChannel) => {
        props.onSetChannel(channel);
    };

    const renderUser = (user: User) => (
        <li key={user.session} className={user.is_me ? styles["me"] : ""}>
            {user.is_speaking ? "● " : "◯ "}
            {user.name} (channel {user.channel_id})
            {user.self_mute || user.mute ? " muted" : ""}
            {user.self_deaf || user.deaf ? " deafened" : ""}
        </li>
    );

    const onChannelContextMenu = async (channel: MumbleChannel, event: React.MouseEvent) => {
        event.preventDefault();

        const menu = await Menu.new({
            items: [
                await MenuItem.new({
                    id: "join-channel",
                    text: `Join ${channel.name}`,
                    action: () => {
                        onSetChannel(channel);
                    }
                })
            ]
        });
        void menu.popup(new LogicalPosition(event.clientX, event.clientY));
    };

    const renderChannel = (channel: MumbleChannel) => {
        const isMeChannel = channel.channel_id === users.find(u => u.is_me)?.channel_id;

        return (
            <li key={channel.channel_id} onDoubleClick={() => onSetChannel(channel)} onContextMenu={(e) => onChannelContextMenu(channel, e)}>
                <span className={isMeChannel ? styles["me-channel"] : ""}>{channel.name} (parent {channel.parent ?? "none"})</span>

                <ul>
                    {users.filter(u => u.channel_id === channel.channel_id).map(u => renderUser(u))}
                </ul>
            </li>
        );
    };

    return (
        <>
            <main className={[styles.container, "container"].join(" ")}>
                <section className={styles["user-list"]}>
                    <section>
                        <ul>
                            {channels.map((channel) => renderChannel(channel))}
                        </ul>
                    </section>
                    <menu>
                        <button type="button" onClick={() => {
                            props.onSetSelfMute(!selfMuted);
                            setSelfMuted(!selfMuted);
                        }}>
                            {selfMuted ? "Unmute" : "Mute"}
                        </button>
                        <button type="button" onClick={props.onDisconnect}>
                            Disconnect
                        </button>
                    </menu>
                </section>
                <section className={styles.chat}>
                    Chat

                    <ol onClick={openChatLink} onAuxClick={openChatLink}>
                        {chatMessages.map((message, index) => {
                            if (message.type === "ServerTextMessage") {
                                return (
                                    <li key={index}>
                                        <span className={styles.date}>[{new Date(message.timestamp).toLocaleTimeString()}]</span>
                                        <span className="content" dangerouslySetInnerHTML={{ __html: message.content }}></span>
                                    </li>
                                );
                            }

                            const user = users.find(u => u.session === message.user_session);
                            return (
                                <li key={index}>
                                    <span className={styles.date}>[{new Date(message.timestamp).toLocaleTimeString()}]</span>
                                    <span className={styles.user}>{user ? user.name : "Unknown"}:</span>
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