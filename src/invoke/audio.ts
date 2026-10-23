import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import z from "zod";

const AudioDeviceSchema = z.object({
    id: z.string(),
    name: z.string(),
    is_default: z.boolean(),
});
export type AudioDevice = z.infer<typeof AudioDeviceSchema>;

const AudioDevicesSchema = z.array(AudioDeviceSchema);
// The device the backend prefers, or null to follow the system default.
const SelectedAudioDeviceSchema = AudioDeviceSchema.nullable();

const INPUT_DEVICES_EVENT = "audio-input-devices-changed";
const OUTPUT_DEVICES_EVENT = "audio-output-devices-changed";
const INPUT_SELECTED_EVENT = "audio-input-device-selected";
const OUTPUT_SELECTED_EVENT = "audio-output-device-selected";
const NOISE_SUPPRESSION_EVENT = "audio-noise-suppression-changed";
// Backend events sent before a listener registers are lost, so announcing
// ourselves makes it publish the current lists and selections.
const SUBSCRIBE_EVENT = "audio-devices-subscribe";

export interface AudioDeviceHandlers {
    onInputs: (devices: AudioDevice[]) => void;
    onOutputs: (devices: AudioDevice[]) => void;
    /** The id of the preferred device, or null for the system default. */
    onInputSelected: (deviceId: string | null) => void;
    onOutputSelected: (deviceId: string | null) => void;
    /** Whether mic noise suppression is on. */
    onNoiseSuppression: (enabled: boolean) => void;
}

/**
 * Calls the handlers with the current device lists and selections, then again
 * whenever one changes. Resolves once subscribed, to a function that stops listening.
 */
export async function subscribeAudioDevices(handlers: AudioDeviceHandlers): Promise<() => void> {
    // An event callback has no caller to throw to, so a bad payload is logged.
    const deliver = <T>(schema: z.ZodType<T>, handler: (value: T) => void) =>
        (event: { payload: unknown }) => {
            const parsed = schema.safeParse(event.payload);
            if (parsed.success) {
                handler(parsed.data);
            } else {
                console.error("Ignoring malformed audio event:", parsed.error);
            }
        };
    const selected = (handler: (deviceId: string | null) => void) =>
        deliver(SelectedAudioDeviceSchema, (device) => handler(device?.id ?? null));

    const stops = [
        await listen<unknown>(INPUT_DEVICES_EVENT, deliver(AudioDevicesSchema, handlers.onInputs)),
        await listen<unknown>(OUTPUT_DEVICES_EVENT, deliver(AudioDevicesSchema, handlers.onOutputs)),
        await listen<unknown>(INPUT_SELECTED_EVENT, selected(handlers.onInputSelected)),
        await listen<unknown>(OUTPUT_SELECTED_EVENT, selected(handlers.onOutputSelected)),
        await listen<unknown>(NOISE_SUPPRESSION_EVENT, deliver(z.boolean(), handlers.onNoiseSuppression)),
    ];
    await emit(SUBSCRIBE_EVENT);
    return () => stops.forEach((stop) => stop());
}

/** `null` goes back to the system default. */
export async function setAudioInputDevice(deviceId: string | null): Promise<void> {
    return invoke("set_audio_input_device", { deviceId });
}
/** `null` goes back to the system default. */
export async function setAudioOutputDevice(deviceId: string | null): Promise<void> {
    return invoke("set_audio_output_device", { deviceId });
}
export async function setNoiseSuppression(enabled: boolean): Promise<void> {
    return invoke("set_noise_suppression", { enabled });
}
