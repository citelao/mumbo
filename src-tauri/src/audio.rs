use std::str::FromStr;

use cpal::{DeviceId, traits::{DeviceTrait, HostTrait, StreamTrait}};
use mumbo_audio::{
    PipelineError,
    mic::{DESIRED_INPUT_SAMPLE_RATE, MicFrameSink, MicPipeline, apm_config, audio_processing},
    speaker::{PLAYBACK_QUEUE_SAMPLES, SPEAKER_SAMPLE_RATE, SpeakerSource, top_up_output},
};
use ringbuf::{HeapRb, traits::{Consumer, Observer, Producer, Split}};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    // "As stable as possible" ID (e.g. across reboots)
    pub id: String,

    // Human-readable name of the device
    pub name: String,

    // Whether this device is the default input device
    pub is_default: bool,
}

/// `cpal::default_host()`, made safe to call from short-lived threads on Windows.
///
/// cpal's WASAPI backend creates one process-wide device enumerator inside the
/// COM apartment of whichever thread asks first, and tears that apartment down
/// when the thread exits. Any later thread then calls into a dead object and
/// crashes (STATUS_ACCESS_VIOLATION). Tests hit this because each runs on its
/// own thread. So create the enumerator on a thread that never exits.
fn default_host() -> cpal::Host {
    #[cfg(windows)]
    {
        static ENUMERATOR_OWNER: std::sync::Once = std::sync::Once::new();
        ENUMERATOR_OWNER.call_once(|| {
            let (ready, wait_until_ready) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("cpal-enumerator-owner".into())
                .spawn(move || {
                    // Any default-device lookup creates the enumerator.
                    let _ = cpal::default_host().default_input_device();
                    let _ = ready.send(());
                    loop {
                        std::thread::park();
                    }
                })
                .expect("spawning the enumerator owner thread");
            let _ = wait_until_ready.recv();
        });
    }
    cpal::default_host()
}

pub fn get_audio_input_devices() -> Result<Vec<AudioDevice>, cpal::Error> {
    let host = default_host();

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
    let host = default_host();

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

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("No audio device available")]
    NoAudioDevice,

    #[error("Unknown audio device: {0}")]
    UnknownDevice(String),

    #[error("CPAL error: {0}")]
    CpalError(#[from] cpal::Error),

    #[error("Audio processing error: {0}")]
    SonoraError(#[from] sonora::Error),

    #[error(transparent)]
    Pipeline(#[from] PipelineError),
}

/// Room for ~200 ms of queued playback.
const SPEAKER_HEADROOM_SAMPLES: usize = SPEAKER_SAMPLE_RATE as usize / 1000 * 200;

/// The preferred device if it is connected, otherwise the system default.
fn resolve_device(
    host: &cpal::Host,
    preferred: Option<&AudioDevice>,
    default: Option<cpal::Device>,
) -> Option<cpal::Device> {
    let Some(preferred) = preferred else {
        return default;
    };
    let found = DeviceId::from_str(&preferred.id)
        .ok()
        .and_then(|id| host.device_by_id(&id));
    if found.is_none() {
        log::warn!("Preferred audio device {:?} is not available; using the default", preferred.name);
    }
    found.or(default)
}

/// Prefers 48 kHz f32, since playback is not resampled.
fn output_stream_config(device: &cpal::Device) -> Result<cpal::StreamConfig, AudioError> {
    let preferred = device
        .supported_output_configs()?
        .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
        .find_map(|c| c.try_with_sample_rate(SPEAKER_SAMPLE_RATE));
    let config = match preferred {
        Some(supported) => supported.config(),
        None => device.default_output_config()?.config(),
    };
    if config.sample_rate != SPEAKER_SAMPLE_RATE {
        // TODO: resample playback; at the wrong rate it plays at the wrong speed.
        log::warn!("Speakers run at {} Hz, not {SPEAKER_SAMPLE_RATE} Hz", config.sample_rate);
    }
    Ok(config)
}


/// A device the user prefers, as saved between runs. Only what is needed to
/// find it again (`id`) and to name it while it is unplugged (`name`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedDevice {
    pub id: String,
    pub name: String,
}

impl From<&AudioDevice> for SavedDevice {
    fn from(device: &AudioDevice) -> Self {
        Self { id: device.id.clone(), name: device.name.clone() }
    }
}

impl From<SavedDevice> for AudioDevice {
    fn from(saved: SavedDevice) -> Self {
        // Whether it is the system default is only known by listing devices.
        Self { id: saved.id, name: saved.name, is_default: false }
    }
}

/// The devices the user prefers; `None` follows the system default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPreferences {
    #[serde(default)]
    pub input: Option<SavedDevice>,
    #[serde(default)]
    pub output: Option<SavedDevice>,
    /// Whether RNNoise cleans up the mic. On unless the user turned it off.
    #[serde(default = "default_noise_suppression")]
    pub noise_suppression: bool,
}

fn default_noise_suppression() -> bool {
    true
}

impl Default for SavedPreferences {
    fn default() -> Self {
        Self { input: None, output: None, noise_suppression: default_noise_suppression() }
    }
}

impl SavedPreferences {
    pub fn of(input: Option<&AudioDevice>, output: Option<&AudioDevice>, noise_suppression: bool) -> Self {
        Self { input: input.map(SavedDevice::from), output: output.map(SavedDevice::from), noise_suppression }
    }

    /// Reads what an earlier run saved. Anything missing or unreadable counts as
    /// nothing saved (the system defaults), so a bad settings file never stops startup.
    pub fn from_stored(value: Option<serde_json::Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        serde_json::from_value(value).unwrap_or_else(|e| {
            log::warn!("Ignoring unreadable saved audio devices: {e}");
            Self::default()
        })
    }
}

pub struct AudioModule {
    // The latest audio devices! Eventually we can update the list live, but for
    // now it is loaded once at startup.
    input_devices: tokio::sync::watch::Sender<Vec<AudioDevice>>,
    preferred_input_device: tokio::sync::watch::Sender<Option<AudioDevice>>,

    output_devices: tokio::sync::watch::Sender<Vec<AudioDevice>>,
    preferred_output_device: tokio::sync::watch::Sender<Option<AudioDevice>>,

    /// Read for every mic frame, so toggling it applies to a call in progress.
    noise_suppression: tokio::sync::watch::Sender<bool>,
}


fn find_device(devices: &[AudioDevice], id: &str) -> Result<AudioDevice, AudioError> {
    devices
        .iter()
        .find(|device| device.id == id)
        .cloned()
        .ok_or_else(|| AudioError::UnknownDevice(id.to_string()))
}

impl AudioModule {
    pub fn new(saved: SavedPreferences) -> Result<Self, AudioError> {
        let input_devices = get_audio_input_devices()?;
        let output_devices = get_audio_output_devices()?;

        let (input_devices_tx, _) = tokio::sync::watch::channel(input_devices);
        let (output_devices_tx, _) = tokio::sync::watch::channel(output_devices);

        let (preferred_input_device_tx, _) = tokio::sync::watch::channel(saved.input.map(AudioDevice::from));
        let (preferred_output_device_tx, _) = tokio::sync::watch::channel(saved.output.map(AudioDevice::from));

        let (noise_suppression_tx, _) = tokio::sync::watch::channel(saved.noise_suppression);

        Ok(AudioModule {
            input_devices: input_devices_tx,
            preferred_input_device: preferred_input_device_tx,
            output_devices: output_devices_tx,
            preferred_output_device: preferred_output_device_tx,
            noise_suppression: noise_suppression_tx,
        })
    }

    pub fn noise_suppression(&self) -> tokio::sync::watch::Receiver<bool> {
        self.noise_suppression.subscribe()
    }

    pub fn set_noise_suppression(&self, enabled: bool) {
        self.noise_suppression.send_replace(enabled);
    }

    pub fn input_devices(&self) -> tokio::sync::watch::Receiver<Vec<AudioDevice>> {
        self.input_devices.subscribe()
    }

    pub fn preferred_input_device(&self) -> tokio::sync::watch::Receiver<Option<AudioDevice>> {
        self.preferred_input_device.subscribe()
    }

    pub fn set_preferred_input_device(&self, device: Option<AudioDevice>) {
        // `send` errors when nothing is subscribed, which is a normal state here.
        self.preferred_input_device.send_replace(device);
        // TODO: rebuild the stream if audio is already running.
    }

    /// Prefers the listed input device with this `id` the next time audio
    /// starts. `None` goes back to following the system default.
    pub fn select_input_device(&self, id: Option<&str>) -> Result<(), AudioError> {
        let device = id.map(|id| find_device(&self.input_devices.borrow(), id)).transpose()?;
        self.set_preferred_input_device(device);
        Ok(())
    }

    /// Prefers the listed output device with this `id`. `None` goes back to
    /// following the system default.
    pub fn select_output_device(&self, id: Option<&str>) -> Result<(), AudioError> {
        let device = id.map(|id| find_device(&self.output_devices.borrow(), id)).transpose()?;
        self.set_preferred_output_device(device);
        Ok(())
    }

    pub fn output_devices(&self) -> tokio::sync::watch::Receiver<Vec<AudioDevice>> {
        self.output_devices.subscribe()
    }

    pub fn preferred_output_device(&self) -> tokio::sync::watch::Receiver<Option<AudioDevice>> {
        self.preferred_output_device.subscribe()
    }

    pub fn set_preferred_output_device(&self, device: Option<AudioDevice>) {
        self.preferred_output_device.send_replace(device);
        // TODO: rebuild the stream if audio is already running.
    }

    pub fn start(&self, mut on_mic_frame: impl MicFrameSink + 'static, mut speakers: impl SpeakerSource + 'static) -> Result<impl Future<Output = Result<(), AudioError>> + 'static, AudioError> {
        let host = default_host();
        let input_device = resolve_device(&host, self.preferred_input_device.borrow().as_ref(), host.default_input_device())
            .ok_or(AudioError::NoAudioDevice)?;
        let output_device = resolve_device(&host, self.preferred_output_device.borrow().as_ref(), host.default_output_device())
            .ok_or(AudioError::NoAudioDevice)?;
        let output_config = output_stream_config(&output_device)?;
        let output_channels = output_config.channels as usize;
        let (mut output_producer, mut output_consumer) = HeapRb::<i16>::new(SPEAKER_HEADROOM_SAMPLES).split();

        // TODO: better config detection. e.g. handling arbitrary sample rates & channel counts.
        let input_config = input_device
            .supported_input_configs()?
            .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
            .find_map(|c| c.try_with_sample_rate(DESIRED_INPUT_SAMPLE_RATE));
        let input_config = match input_config {
            Some(supported) => supported.config(),
            None => input_device
                .default_input_config()?
                .config(),
        };

        let input_channels = input_config.channels;
        let input_rate = input_config.sample_rate;

        // Allocate enough headroom for ~200ms of audio.
        const HEADROOM_MS: u32 = 200;
        let headroom_samples: u32 = (input_rate * HEADROOM_MS) / 1000;
        let (mut input_producer, mut input_consumer) = HeapRb::<f32>::new(headroom_samples as usize).split();

        let apm_config = apm_config();
        let mut apm = audio_processing(apm_config.clone());
        let noise_suppression = self.noise_suppression.subscribe();

        // A best-guess for the network-induced delay.
        // TODO: measure actual network delay and update this value accordingly.
        apm.set_stream_delay_ms(50)?;

        log::info!("Audio input: {:?}, {input_config:?}", input_device.description()?);
        log::info!("Audio output: {:?}, {output_config:?}", output_device.description()?);
        log::info!("Audio processing config: {apm_config:?}");
        if input_config.sample_rate != DESIRED_INPUT_SAMPLE_RATE {
            log::warn!("Mic runs at {} Hz; resampling to {DESIRED_INPUT_SAMPLE_RATE} Hz for Opus", input_config.sample_rate);
        }

        let task = async move {
            let mic_stream = input_device.build_input_stream(
                input_config,
                move |input: &[f32], _| {
                    for frame in input.chunks(input_channels as usize) {
                        let mono = frame.iter().sum::<f32>() / input_channels as f32;
                        let _ = input_producer.try_push(mono);
                    }
                },
                |err| log::error!("mic stream error: {err}"),
                None,
            )?;
            mic_stream.play()?;

            // Playback stops when `speaker_stream` is dropped, so it lives for the whole loop.
            let speaker_stream = output_device.build_output_stream(
                output_config,
                move |output: &mut [f32], _| {
                    // same value in every channel
                    for frame in output.chunks_mut(output_channels) {
                        let sample = output_consumer.try_pop().unwrap_or(0) as f32 / i16::MAX as f32;
                        frame.fill(sample);
                    }
                },
                |err| log::error!("speaker stream error: {err}"),
                None,
            )?;
            speaker_stream.play()?;

            let mut pipeline = MicPipeline::new(input_rate)?;
            // Captured mic audio at the device rate.
            let mut captured = Vec::new();

            let mut mix_interval = tokio::time::interval(std::time::Duration::from_millis(5));
            loop {
                tokio::select! {
                    _ = mix_interval.tick() => {
                        // Top the ring buffer up to a fixed level rather than mixing once per
                        // tick: the card drains it at its own rate, so this follows the card's
                        // clock instead of drifting against this timer.
                        if !top_up_output(&mut output_producer, PLAYBACK_QUEUE_SAMPLES, &mut speakers, &mut apm)? {
                            log::info!("Nothing left to play; stopping audio");
                            break;
                        }

                        captured.resize(input_consumer.occupied_len(), 0.0);
                        input_consumer.pop_slice(&mut captured);
                        pipeline.process(&captured, &mut apm, *noise_suppression.borrow(), &mut on_mic_frame)?;
                    }
                }
            }

            Ok::<(), AudioError>(())
        };

        Ok(task)
    }
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
    fn saved_preferences_round_trip_through_json() {
        let selected = AudioDevice { id: "mic".into(), name: "Dock Mic".into(), is_default: true };
        let saved = SavedPreferences::of(Some(&selected), None, false);

        let json = serde_json::to_value(&saved).unwrap();
        // is_default describes the moment it was saved, so it is not kept.
        assert_eq!(json, serde_json::json!({ "input": { "id": "mic", "name": "Dock Mic" }, "output": null, "noise_suppression": false }));

        assert_eq!(SavedPreferences::from_stored(Some(json)), saved);
    }

    #[test]
    fn noise_suppression_is_on_unless_turned_off() {
        assert!(SavedPreferences::default().noise_suppression);
        assert!(SavedPreferences::from_stored(Some(serde_json::json!({ "input": null }))).noise_suppression);
        assert!(!SavedPreferences::from_stored(Some(serde_json::json!({ "noise_suppression": false }))).noise_suppression);
    }

    #[test]
    fn nothing_saved_means_the_system_default() {
        assert_eq!(SavedPreferences::from_stored(None), SavedPreferences::default());
        assert_eq!(SavedPreferences::from_stored(Some(serde_json::json!({}))), SavedPreferences::default());
    }

    #[test]
    fn unreadable_saved_preferences_count_as_nothing_saved() {
        for garbage in [
            serde_json::json!("nonsense"),
            serde_json::json!(5),
            serde_json::json!({ "input": 5 }),
            serde_json::json!({ "input": { "id": "no name" } }),
            serde_json::json!({ "input": { "id": 1, "name": "x" } }),
        ] {
            assert_eq!(SavedPreferences::from_stored(Some(garbage.clone())), SavedPreferences::default(), "{garbage}");
        }
    }

    #[test]
    fn a_restored_preference_survives_its_device_being_unplugged() {
        let saved = SavedPreferences {
            input: Some(SavedDevice { id: "dock-mic".into(), name: "Dock Mic".into() }),
            output: None,
            noise_suppression: true,
        };

        let module = AudioModule::new(saved).unwrap();

        // Not necessarily in the device list (the dock may be unplugged), but still preferred.
        assert!(module.input_devices.borrow().iter().all(|d| d.id != "dock-mic"));
        assert_eq!(preferred_id(&module.preferred_input_device).as_deref(), Some("dock-mic"));
        assert_eq!(preferred_id(&module.preferred_output_device), None);
    }

    fn device(id: &str) -> AudioDevice {
        AudioDevice { id: id.into(), name: id.into(), is_default: false }
    }

    fn module_with(inputs: Vec<AudioDevice>, outputs: Vec<AudioDevice>) -> AudioModule {
        let module = AudioModule::new(SavedPreferences::default()).unwrap();
        module.input_devices.send_replace(inputs);
        module.output_devices.send_replace(outputs);
        module
    }

    fn preferred_id(preferred: &tokio::sync::watch::Sender<Option<AudioDevice>>) -> Option<String> {
        preferred.borrow().as_ref().map(|d| d.id.clone())
    }

    #[test]
    fn selecting_a_listed_input_device_makes_it_the_preference() {
        let module = module_with(vec![device("a"), device("b")], vec![]);

        module.select_input_device(Some("b")).unwrap();

        assert_eq!(preferred_id(&module.preferred_input_device).as_deref(), Some("b"));
    }

    #[test]
    fn selecting_a_listed_output_device_makes_it_the_preference() {
        let module = module_with(vec![], vec![device("x"), device("y")]);

        module.select_output_device(Some("y")).unwrap();

        assert_eq!(preferred_id(&module.preferred_output_device).as_deref(), Some("y"));
        assert_eq!(preferred_id(&module.preferred_input_device), None);
    }

    #[test]
    fn selecting_an_unlisted_device_fails_and_keeps_the_preference() {
        let module = module_with(vec![device("a")], vec![device("x")]);
        module.select_input_device(Some("a")).unwrap();

        assert!(matches!(module.select_input_device(Some("nope")), Err(AudioError::UnknownDevice(id)) if id == "nope"));
        assert!(matches!(module.select_output_device(Some("a")), Err(AudioError::UnknownDevice(_))), "inputs are not outputs");

        assert_eq!(preferred_id(&module.preferred_input_device).as_deref(), Some("a"));
        assert_eq!(preferred_id(&module.preferred_output_device), None);
    }

    #[test]
    fn selecting_no_input_device_goes_back_to_the_default() {
        let module = module_with(vec![device("a")], vec![]);
        module.select_input_device(Some("a")).unwrap();

        module.select_input_device(None).unwrap();

        assert_eq!(preferred_id(&module.preferred_input_device), None);
    }

    #[test]
    fn selecting_no_output_device_goes_back_to_the_default() {
        let module = module_with(vec![], vec![device("x")]);
        module.select_output_device(Some("x")).unwrap();

        module.select_output_device(None).unwrap();

        assert_eq!(preferred_id(&module.preferred_output_device), None);
    }

    #[test]
    fn the_preference_can_change_again() {
        let module = module_with(vec![device("a"), device("b")], vec![]);

        module.select_input_device(Some("a")).unwrap();
        module.select_input_device(Some("b")).unwrap();

        assert_eq!(preferred_id(&module.preferred_input_device).as_deref(), Some("b"));
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
