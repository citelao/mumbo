import { assembleChannelTree, TreeNode } from "../helpers/tree";
import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Channel, fetchChannels, fetchChatMessages, fetchUsers, TextMessage, type User } from "../invoke/mumbleState";
import { ProtocolViewer, type ProtocolLogEntry } from "../ProtocolViewer";
import styles from "./MainPage.module.scss";
import type { Channel as MumbleChannel } from "../invoke/mumbleState";
import { Menu, MenuItem } from "@tauri-apps/api/menu";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { useEditor, EditorContent } from '@tiptap/react';
import StarterKit from '@tiptap/starter-kit';
import { Extension } from '@tiptap/core';

function createCustomEnterExtension(onEnter: (message: string) => void) {
    const ext = Extension.create({
      name: 'customEnter',
    
      addKeyboardShortcuts() {
        return {
          'Enter': () => {
            onEnter(this.editor.getText());
            return true; 
          },
          'Shift-Enter': () => {
            // Handle Shift+Enter differently (e.g., insert a hard break)
            this.editor.commands.setHardBreak();
            return true;
          },
        };
      },
    });
    return ext;
}


export interface MainPageProps {
    protocolLog: ProtocolLogEntry[];
    onClearProtocolLog: () => void;

    onDisconnect: () => void;
    onSendMessage: (message: string) => void;
    onSetSelfMute: (muted: boolean) => void;
    onSetChannel: (channel: MumbleChannel) => void;
    onSendUserMessage: (message: string, session: number[]) => void;
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

    const onSetChannel = (channel: MumbleChannel) => {
        props.onSetChannel(channel);
    };

    const onSendUserMessage = (message: string, session: number[]) => {
        props.onSendUserMessage(message, session);
    };

    const onUserContextMenu = async (
        user: User,
        event: React.MouseEvent
    ) => {
        event.preventDefault();

        // TODO: real actions.
        const menu = await Menu.new({
            items: [
                await MenuItem.new({
                    id: "annoy",
                    text: "annoy",
                    action: () => {
                        console.log(`Annoying user ${user.name}`);
                        onSendUserMessage(`Annoying you!`, [user.session]);
                    }
                })
            ]
        });
        void menu.popup(new LogicalPosition(event.clientX, event.clientY));
    };

    const renderUser = (user: User) => (
        <li key={user.session}
            onContextMenu={(e) => onUserContextMenu(user, e)}
            onDoubleClick={() => onSendUserMessage(`Hello ${user.name}!`, [user.session])}
            className={[styles["user"], user.is_me ? styles["me"] : ""].join(" ")}>
            <span>
                {user.is_speaking ? "● " : "◯ "}
                {user.name}
            </span>
            <span>
                {user.self_mute || user.mute ? " muted" : ""}
                {user.self_deaf || user.deaf ? " deafened" : ""}
            </span>
        </li>
    );

    const onChannelContextMenu = async (
        channel: MumbleChannel,
        is_current: boolean,
        event: React.MouseEvent) => {
        event.preventDefault();

        const joinItem = is_current ? null : await MenuItem.new({
            id: "join-channel",
            text: `Join ${channel.name}`,
            action: () => {
                onSetChannel(channel);
            }
        });

        const menu = await Menu.new({
            items: [
                ...(joinItem ? [joinItem] : [])
            ]
        });
        void menu.popup(new LogicalPosition(event.clientX, event.clientY));
    };

    const renderChannel = (channel: MumbleChannel, childChannels: MumbleChannel[]) => {
        const isMeChannel = channel.channel_id === users.find(u => u.is_me)?.channel_id;

        return (
            <li key={channel.channel_id}>
                <span className={[styles["channel"], isMeChannel ? styles["me-channel"] : ""].join(" ")}
                    onDoubleClick={() => onSetChannel(channel)}
                    onContextMenu={(e) => onChannelContextMenu(channel, isMeChannel, e)}>
                        <span>{channel.name}</span>
                        {/* <span>{channel.is_enter_restricted ? " 🔒" : ""}</span> */}
                        <span>{channel.is_enter_restricted ? " restricted" : ""}</span>
                </span>

                <ul>
                    {users.filter(u => u.channel_id === channel.channel_id).map(u => renderUser(u))}
                </ul>

                {childChannels.length > 0 && (
                    <ul>
                        {childChannels.map(child => renderChannel(child, channels.filter(c => c.parent === child.channel_id)))}
                    </ul>
                )}
            </li>
        );
    };

    const channelTree = channels.length > 0 ? assembleChannelTree(channels) : null;
    const renderChannelTree = (node: TreeNode<MumbleChannel>) => {
        return renderChannel(node.value, node.children.map(child => child.value));
    };

    const sendCurrentMessage = () => {
        const message_text = editor?.getText();
        const message = editor?.getHTML();
        if (message_text.trim().length > 0) {
            props.onSendMessage(message.trim());
            editor?.commands.clearContent();
        }
    };

    const enterExtension = useMemo(() => createCustomEnterExtension(sendCurrentMessage), []);

    const editor = useEditor({
        extensions: [
            StarterKit,
            enterExtension,
            // Document.extend({
            //     addKeyboardShortcuts() {
            //         return {
            //             "Mod-Enter": () => {
            //                 sendCurrentMessage();
            //                 return true;
            //             }
            //         };
            //     }
            // }),
        ],
        content: 'Hello World!', // initial content
    });

    return (
        <>
            <main className={[styles.container, "container"].join(" ")}>
                <section className={styles["user-list"]}>
                    <section>
                        <ul>
                            {channelTree && renderChannelTree(channelTree)}
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

                            const user = users.find(u => u.session === message.actor);
                            const recipients = message.session.map(s => users.find(u => u.session === s));
                            const recipientsString = recipients.length === 0
                                ? ""
                                : ` to ${recipients.map(r => r ? r.name : "Unknown").join(" & ")}`
                            return (
                                <li key={index}>
                                    <span className={styles.date}>[{new Date(message.timestamp).toLocaleTimeString()}]</span>
                                    <span className={styles.user}>{user ? user.name : "Unknown"}{recipientsString}:</span>
                                    <span className="content" dangerouslySetInnerHTML={{ __html: message.content }}></span>
                                </li>
                            );
                        })}
                    </ol>
                    {/* <label>
                        Chat
                        <input type="text" placeholder="Message" onKeyUp={(e) => {
                            if (e.key === "Enter") {
                                e.currentTarget.form?.dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
                            }
                        }} />
                    </label> */}

                    <EditorContent editor={editor} className={styles.editor} onKeyUp={(e) => {
                        if (e.key === "Enter" && !e.shiftKey) {
                            sendCurrentMessage();
                        }
                    }} />
                </section>
            </main>
            <ProtocolViewer entries={props.protocolLog} onClear={props.onClearProtocolLog} />
        </>
    );
}