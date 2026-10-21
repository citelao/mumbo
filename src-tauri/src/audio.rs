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