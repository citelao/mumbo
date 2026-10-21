import { useState, useEffect } from "react";
import { getServerStats } from "../invoke";
import type { ServerStats } from "../invoke";
import "./LoginPage.scss";

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

    recentServers?: { name?: string; server?: string; port?: number; password?: string }[];

    onConnect: (params: OnConnectParams) => Promise<OnConnectResult>;
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
            {/* Form for connection */}
            {props.recentServers && props.recentServers.length > 0 && (
                <div className="recent-servers">
                    <h3>Recent Servers</h3>
                    <ul>
                        {props.recentServers.map((s, index) => (
                            <li key={index}>
                                <button onClick={async (e) => {
                                    e.preventDefault();
                                    await connectToServer({ name: s.name!, server: s.server!, port: s.port!, password: s.password! });
                                }}>
                                    {s.name} ({s.server}:{s.port})
                                </button>
                            </li>
                        ))}
                    </ul>
                </div>
            )}

            <form className="connection-form"
                onSubmit={async (e) => {
                    e.preventDefault();
                    await connectToServer({ name, server, port, password });
                }}>
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