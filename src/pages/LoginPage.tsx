import { useState, useEffect, useCallback } from "react";
import { getServerStats } from "../invoke";
import type { ServerStats } from "../invoke";
import { setAudioInputDevice, setAudioOutputDevice, setNoiseSuppression, subscribeAudioDevices } from "../invoke/audio";
import type { AudioDevice } from "../invoke/audio";
import styles from "./LoginPage.module.scss";
import { Server } from "../settings";
import { throttled, useInterval, useThrottledEffect } from "../helpers/throttle";

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

    // Refresh server stats, throttled to avoid server load.
    const THROTTLE_MS = 2000;
    const refreshServerStats = useCallback(throttled(async () => {
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
    }, THROTTLE_MS), [server, port]);

    const REFRESH_STATS_MS = 10_000;
    useInterval(refreshServerStats, REFRESH_STATS_MS);

    // The backend owns the preference (it outlives this page); null follows the system default.
    const [selectedAudioInputId, setSelectedAudioInputId] = useState<string | null>(null);
    const [selectedAudioOutputId, setSelectedAudioOutputId] = useState<string | null>(null);
    const [audioInputDevices, setAudioInputDevices] = useState<AudioDevice[]>([]);
    const [audioOutputDevices, setAudioOutputDevices] = useState<AudioDevice[]>([]);
    // On unless the backend says otherwise, like the backend's own default.
    const [noiseSuppression, setNoiseSuppressionEnabled] = useState(true);
    useEffect(() => {
        const subscription = subscribeAudioDevices({
            onInputs: setAudioInputDevices,
            onOutputs: setAudioOutputDevices,
            onInputSelected: setSelectedAudioInputId,
            onOutputSelected: setSelectedAudioOutputId,
            onNoiseSuppression: setNoiseSuppressionEnabled,
        });
        subscription.catch((err) => console.error("Failed to subscribe to audio devices:", err));
        return () => {
            subscription.then(stop => stop(), () => {});
        };
    }, []);

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
        <main className={[styles.container, "container"].join(" ")}>
            <div className={styles["sub-container"]}>
                {props.recentServers && props.recentServers.length > 0 && (
                    <div className={styles["recent-servers"]}>
                        <h3>Recent servers</h3>
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
                <form className={styles["connection-form"]}
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
                    <div className={styles.footer}>
                        {serverStats && serverStats.stats && (
                            <div  className={[styles["user-count"], error ? styles.error : ""].join(" ")}>
                                {error ?? ""} {serverStats.stats.user_count} / {serverStats.stats.max_user_count} ({serverStats.stats.round_trip_time}ms)
                            </div>
                        ) || (
                            <div className={[styles["user-count"], error ? styles.error : ""].join(" ")}>
                                {error ?? "Fetching..."}
                            </div>
                        )}

                        <button type="submit" disabled={isConnecting}>
                            {isConnecting ? "Connecting..." : "Connect"}
                        </button>
                    </div>
                </form>
            </div>
            <div>
                <h3>Settings</h3>

                <form className={styles.settingsForm}>
                    <label htmlFor="audio-input">Audio input</label>
                    <select id="audio-input" value={selectedAudioInputId ?? ""} onChange={(e) => {
                        const deviceId = e.target.value || null;
                        setSelectedAudioInputId(deviceId);
                        setAudioInputDevice(deviceId)
                            .catch((err) => console.error("Failed to set audio input device:", err));
                    }}>
                        <option value="">Default device</option>
                        {audioInputDevices.map((device) => {
                            const isDefault = device.is_default;
                            return (
                                <option key={device.id} value={device.id}>
                                    {device.name} {isDefault ? "(Default)" : ""}
                                </option>
                            );
                        })}
                    </select>

                    <label htmlFor="audio-output">Audio output</label>
                    <select id="audio-output" value={selectedAudioOutputId ?? ""} onChange={(e) => {
                        const deviceId = e.target.value || null;
                        setSelectedAudioOutputId(deviceId);
                        setAudioOutputDevice(deviceId)
                            .catch((err) => console.error("Failed to set audio output device:", err));
                    }}>
                        <option value="">Default device</option>
                        {audioOutputDevices.map((device) => {
                            const isDefault = device.is_default;
                            return (
                                <option key={device.id} value={device.id}>
                                    {device.name} {isDefault ? "(Default)" : ""}
                                </option>
                            );
                        })}
                    </select>

                    <label htmlFor="noise-suppression" className={styles.checkboxLabel}>Noise suppression</label>
                    <input
                        id="noise-suppression"
                        type="checkbox"
                        checked={noiseSuppression}
                        onChange={(e) => {
                            setNoiseSuppressionEnabled(e.target.checked);
                            setNoiseSuppression(e.target.checked)
                                .catch((err) => console.error("Failed to set noise suppression:", err));
                        }}
                    />
                </form>
            </div>
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

    const fetchServerStats = async () => {
        try {
            const stats = await getServerStats({ server: server.server!, port: server.port! });
            setServerStats(stats);
        } catch (error) {
            setServerStats(null);
        }
    };

    // Limit checking status to every 2s, worst-case (even though this should not refire).
    const [serverStats, setServerStats] = useState<ServerStats | null>(null);
    useThrottledEffect(() => {
        fetchServerStats();
    }, [server.server, server.port], 2000);

    // Also refresh server stats every few s, regardless of throttling.
    useInterval(() => {
        fetchServerStats();
    }, 5_000);

    return (
        <li>
            <button onClick={async (e) => {
                e.preventDefault();
                    await onConnect(server);
                }}>
                {server.name} @ <strong>{server.server}</strong>:{server.port} <br />
                {serverStats ? `${serverStats.user_count} / ${serverStats.max_user_count}` : "Fetching..."}{" "}
                {serverStats ? `(${serverStats.round_trip_time}ms)` : ""}
            </button>
            <button
                className={styles.remove}
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