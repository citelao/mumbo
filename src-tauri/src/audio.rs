use cpal::traits::{HostTrait, DeviceTrait};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    // "As stable as possible" ID (e.g. across reboots)
    pub id: String,

    // Human-readable name of the device
    pub name: String,

    // Whether this device is the default input device
    pub is_default: bool,
}

pub fn get_audio_input_devices() -> Result<Vec<AudioDevice>, cpal::Error> {
    let host = cpal::default_host();

    let default_device = host.default_input_device();
    let default_device_id = default_device.and_then(|d| d.id().ok());

    let devices = host.input_devices()?;
    let device_names = devices.filter_map(|d| {
        let desc = d.description().ok()?;
        let name = desc.name();
        let id = d.id().ok()?;
        let id_str = id.to_string();

        Some(AudioDevice {
            id: id_str,
            name: name.to_string(),
            is_default: Some(&id) == default_device_id.as_ref(),
        })
    }).collect();
    Ok(device_names)
}

pub fn get_audio_output_devices() -> Result<Vec<AudioDevice>, cpal::Error> {
    let host = cpal::default_host();

    let default_device = host.default_output_device();
    let default_device_id = default_device.and_then(|d| d.id().ok());

    let devices = host.output_devices()?;
    let device_names = devices.filter_map(|d| {
        let desc = d.description().ok()?;
        let name = desc.name();
        let id = d.id().ok()?;
        let id_str = id.to_string();

        Some(AudioDevice {
            id: id_str,
            name: name.to_string(),
            is_default: Some(&id) == default_device_id.as_ref(),
        })
    }).collect();
    Ok(device_names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // These hit the real audio host, so CI machines may have zero devices;
    // assert invariants that hold for any device list (including empty).
    fn check(devices: &[AudioDevice]) {
        let ids: HashSet<_> = devices.iter().map(|d| &d.id).collect();
        assert_eq!(ids.len(), devices.len(), "device ids must be unique");
        assert!(devices.iter().all(|d| !d.id.is_empty() && !d.name.is_empty()));
        let defaults = devices.iter().filter(|d| d.is_default).count();
        // Not "exactly one when any exist": ALSA on headless Linux lists devices
        // but reports a default that isn't among them.
        assert!(defaults <= 1, "at most one device may be the default");
    }

    #[test]
    fn input_devices_are_well_formed() {
        check(&get_audio_input_devices().expect("enumerating inputs"));
    }

    #[test]
    fn output_devices_are_well_formed() {
        check(&get_audio_output_devices().expect("enumerating outputs"));
    }

    #[test]
    fn listing_is_stable_between_calls() {
        let a: Vec<_> = get_audio_input_devices().unwrap().into_iter().map(|d| d.id).collect();
        let b: Vec<_> = get_audio_input_devices().unwrap().into_iter().map(|d| d.id).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn serializes_to_the_shape_the_frontend_parses() {
        // src/audioDevices.test.ts parses this same fixture with the zod schema.
        let devices = vec![
            AudioDevice { id: "built-in-mic".into(), name: "Built-in Microphone".into(), is_default: true },
            AudioDevice { id: "usb-mic".into(), name: "USB Microphone".into(), is_default: false },
        ];
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/audio_devices.json")).unwrap();
        assert_eq!(serde_json::to_value(devices).unwrap(), fixture);
    }
}
