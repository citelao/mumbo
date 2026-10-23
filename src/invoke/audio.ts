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

const INPUT_DEVICES_EVENT = "audio-input-devices-changed";
const OUTPUT_DEVICES_EVENT = "audio-output-devices-changed";
// Backend events sent before a listener registers are lost, so announcing
// ourselves makes it publish the current lists.
const SUBSCRIBE_EVENT = "audio-devices-subscribe";

export interface AudioDeviceHandlers {
    onInputs: (devices: AudioDevice[]) => void;
    onOutputs: (devices: AudioDevice[]) => void;
}

/**
 * Calls the handlers with the current device lists, then again whenever a list
 * changes. Resolves once subscribed, to a function that stops listening.
 */
export async function subscribeAudioDevices(handlers: AudioDeviceHandlers): Promise<() => void> {
    // An event callback has no caller to throw to, so a bad payload is logged.
    const deliver = (handler: (devices: AudioDevice[]) => void) =>
        (event: { payload: unknown }) => {
            const parsed = AudioDevicesSchema.safeParse(event.payload);
            if (parsed.success) {
                handler(parsed.data);
            } else {
                console.error("Ignoring malformed audio device list:", parsed.error);
            }
        };

    const stopInputs = await listen<unknown>(INPUT_DEVICES_EVENT, deliver(handlers.onInputs));
    const stopOutputs = await listen<unknown>(OUTPUT_DEVICES_EVENT, deliver(handlers.onOutputs));
    await emit(SUBSCRIBE_EVENT);
    return () => {
        stopInputs();
        stopOutputs();
    };
}
export async function setAudioInputDevice(deviceId: string): Promise<void> {
    return invoke("set_audio_input_device", { deviceId });
}
export async function setAudioOutputDevice(deviceId: string): Promise<void> {
    return invoke("set_audio_output_device", { deviceId });
}
