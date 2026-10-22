import { invoke } from "@tauri-apps/api/core";
import z from "zod";

const AudioDeviceSchema = z.object({
    id: z.string(),
    name: z.string(),
    is_default: z.boolean(),
});
export type AudioDevice = z.infer<typeof AudioDeviceSchema>;
export async function getAudioInputDevices(): Promise<AudioDevice[]> {
    const raw = await invoke("get_audio_input_devices");
    return z.array(AudioDeviceSchema).parse(raw);
}
export async function setAudioInputDevice(deviceId: string): Promise<void> {
    return invoke("set_audio_input_device", { deviceId });
}
export async function getAudioOutputDevices(): Promise<AudioDevice[]> {
    const raw = await invoke("get_audio_output_devices");
    return z.array(AudioDeviceSchema).parse(raw);
}
export async function setAudioOutputDevice(deviceId: string): Promise<void> {
    return invoke("set_audio_output_device", { deviceId });
}
