import { useState, useEffect } from "react";
import { getServerStats } from "../invoke";
import type { ServerStats } from "../invoke";

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

    onConnect: (params: OnConnectParams) => Promise<OnConnectResult>;
    onSaveSettings: (settings: OnConnectParams) => Promise<void>;
}

export default function LoginPage(props: LoginPageProps) {
    const [name, setName] = useState(props.name ?? "testo");
    const [server, setServer] = useState(props.server ?? "localhost");
    const [port, setPort] = useState(props.port ?? 64738);
    const [password, setPassword] = useState(props.password ?? "");

    const [isConnecting, setIsConnecting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const refreshServerStats = async () => {
        try {
            const stats = await getServerStats({ server, port });
            setServerStats(stats);
        } catch (err) {
            console.error("Failed to fetch server stats:", err);
        }
    };

    const [serverStats, setServerStats] = useState<ServerStats | null>(null);

    return (
        <main className="container">
            {/* Form for connection */}
            <form className="connection-form"
                onSubmit={async (e) => {
                    e.preventDefault();
                    setIsConnecting(true);
                    const result = await props.onConnect({ server, port, password, name });
                    setIsConnecting(false);
                    if (!result.succeeded) {
                        setError(result.error);
                    } else {
                        await props.onSaveSettings({ server, port, password, name });
                        setError(null);
                    }
                }}>
                <label>
                    Server
                    <input
                        placeholder="Server"
                        value={server}
                        onChange={(e) => setServer(e.target.value)}
                        onBlur={refreshServerStats} />
                </label>
                <label>
                    Port
                    <input
                        placeholder="Port"
                        value={port}
                        onChange={(e) => setPort(Number(e.target.value))}
                        onBlur={refreshServerStats} />
                </label>
                {serverStats && (
                    <div className="server-stats">
                        <p>Server Version: {serverStats.server_version_v2}</p>
                        <p>User Count: {serverStats.user_count}</p>
                        <p>Max User Count: {serverStats.max_user_count}</p>
                        <p>Max Bandwidth per User: {serverStats.max_bandwidth_per_user}</p>
                    </div>
                )}
                <label>
                    Password
                    <input placeholder="Password" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
                </label>
                <label>
                    Name
                    <input placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} />
                </label>
                <br />
                <button type="submit" disabled={isConnecting}>
                    {isConnecting ? "Connecting..." : "Connect"}
                </button>
                {error && <p className="error">{error}</p>}
            </form>
        </main>
    );
}