import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import z from "zod";

// Mirrors Rust's `DisconnectReason`: why a connection ended.
const DisconnectReasonSchema = z.discriminatedUnion("kind", [
    // The server removed us: an admin kicked or banned us, or we connected
    // again from elsewhere with the same name.
    z.strictObject({
        kind: z.literal("removed"),
        reason: z.string().nullable(),
        // The admin who did it, if anyone.
        by: z.string().nullable(),
        banned: z.boolean(),
    }),
    // The server ended the connection cleanly without saying why.
    z.strictObject({ kind: z.literal("server_closed") }),
    // The connection broke, e.g. because the server went away.
    z.strictObject({ kind: z.literal("connection_lost"), detail: z.string() }),
]);
export type DisconnectReason = z.infer<typeof DisconnectReasonSchema>;

// Mirrors Rust's `Disconnected` event: the connection the UI showed ended.
const DisconnectedSchema = z.strictObject({
    reason: DisconnectReasonSchema,
});

/**
 * Calls `handler` when the connection ends on its own (e.g. the server went
 * away); disconnecting ourselves doesn't fire it. An event that fails the
 * schema is logged and dropped, since there's no caller to throw to.
 */
export function onDisconnected(handler: (reason: DisconnectReason) => void): Promise<UnlistenFn> {
    return listen<unknown>("disconnected", (event) => {
        const result = DisconnectedSchema.safeParse(event.payload);
        if (!result.success) {
            console.error("Disconnect event failed schema validation:", z.prettifyError(result.error), event.payload);
            return;
        }
        handler(result.data.reason);
    });
}
