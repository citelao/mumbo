import { useState, useEffect } from "react";
import { getServerStats } from "../invoke";
import type { ServerStats } from "../invoke";
import "./LoginPage.scss";
import { Server } from "../settings";

export interface OnConnectParams {
    server: string;
    port: number;
    password: string;
    name: string;
}
export type OnConnectResult = {
    succeeded: true;
} | {
    succeeded: false;
    error: string;
}


export interface LoginPageProps {
    // Defaults:
    name?: string;
    server?: string;
    port?: number;
    password?: string;

    recentServers?: Server[];

    onConnect: (params: OnConnectParams) => Promise<OnConnectResult>;
    onRemoveRecentServer: (params: Server) => Promise<void>;
}

export default function LoginPage(props: LoginPageProps) {
    const [name, setName] = useState(props.name ?? "testo");
    const [server, setServer] = useState(props.server ?? "localhost");
    const [port, setPort] = useState(props.port ?? 64738);
    const [password, setPassword] = useState(props.password ?? "");

    const [isConnecting, setIsConnecting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        refreshServerStats();
    }, []);

    const refreshServerStats = async () => {
        // if (serverStats && serverStats.server === server && serverStats.port === port) {
        //     return;
        // }

        try {
            setServerStats({ stats: undefined, server, port });
            const stats = await getServerStats({ server, port });
            setServerStats({ stats, server, port });
            setError(null);
        } catch (err) {
            console.error("Failed to fetch server stats:", err);
            setError("Failed to fetch server stats");
        }
    };

    const [serverStats, setServerStats] = useState<{
        stats?: ServerStats,
        server: string;
        port: number;
    } | null>(null);

    const connectToServer = async (s: { name: string; server: string; port: number; password: string }) => {
        setIsConnecting(true);
        const result = await props.onConnect({
            server: s.server,
            port: s.port,
            password: s.password,
            name: s.name
        });
        setIsConnecting(false);
        if (!result.succeeded) {
            setError(result.error);
        } else {
            setError(null);
        }
    };

    return (
        <main className="container">
            {props.recentServers && props.recentServers.length > 0 && (
                <div className="recent-servers">
                    <h3>Recent Servers</h3>
                    <ul>
                        {props.recentServers.map((s, index) => (
                            <RecentServerButton
                                key={index}
                                server={s}
                                onConnect={(s) => connectToServer({ name: s.name!, server: s.server!, port: s.port!, password: s.password! })}
                                onRemove={props.onRemoveRecentServer}
                            />
                        ))}
                    </ul>
                </div>
            )}

            {/* Form for connection */}
            <form className="connection-form"
                onSubmit={async (e) => {
                    e.preventDefault();
                    await connectToServer({ name, server, port, password });
                }}>
                <h3>Connect to new server</h3>
                <label htmlFor="server">
                    Server
                </label>
                <input
                    id="server"
                    placeholder="Server"
                    value={server}
                    onChange={(e) => setServer(e.target.value)}
                    onBlur={refreshServerStats} />
                <label htmlFor="port">
                    Port
                </label>
                <input
                    id="port"
                    placeholder="Port"
                    value={port}
                    onChange={(e) => setPort(Number(e.target.value))}
                    onBlur={refreshServerStats} />
                <label htmlFor="password">
                    Password
                </label>
                <input id="password" placeholder="Password" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
                <label htmlFor="name">
                    Name
                </label>
                <input id="name" placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} />

                {/* TODO: prevent brief flicker */}
                <div className="footer">
                    {serverStats && serverStats.stats && (
                        <div  className={["user-count", error ? "error" : ""].join(" ")}>
                            {error ?? ""} {serverStats.stats.user_count} / {serverStats.stats.max_user_count}
                        </div>
                    ) || (
                        <div className={["user-count", error ? "error" : ""].join(" ")}>
                            {error ?? "Fetching..."}
                        </div>
                    )}

                    <button type="submit" disabled={isConnecting}>
                        {isConnecting ? "Connecting..." : "Connect"}
                    </button>
                </div>
            </form>
        </main>
    );
}

interface RecentServerButtonProps {
    server: Server;
    onConnect: (server: Server) => Promise<void>;
    onRemove: (server: Server) => Promise<void>;
}

function RecentServerButton(props: RecentServerButtonProps) {
    const { server, onRemove, onConnect } = props;

    const [serverStats, setServerStats] = useState<ServerStats | null>(null);
    useEffect(() => {
        const fetchServerStats = async () => {
            try {
                const stats = await getServerStats({ server: server.server!, port: server.port! });
                setServerStats(stats);
            } catch (error) {
                setServerStats(null);
            }
        };

        fetchServerStats();
    }, [server.server, server.port]);

    return (
        <li>
            <button onClick={async (e) => {
                e.preventDefault();
                    await onConnect(server);
                }}>
                {server.name} @ <strong>{server.server}</strong>:{server.port} <br />
                {serverStats ? `${serverStats.user_count} / ${serverStats.max_user_count}` : "Fetching..."}
            </button>
            <button
                className="remove"
                onClick={async (e) => {
                    e.preventDefault();
                    await onRemove(server);
                }}
                aria-label={`Remove ${server.name} @ ${server.server}:${server.port} from recent servers`}
                title={`Remove ${server.name} @ ${server.server}:${server.port} from recent servers`}>
                &times;
            </button>
        </li>
    );
}