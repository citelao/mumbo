use std::{io::Error, str::FromStr};

use cpal::{DeviceId, traits::{DeviceTrait, HostTrait, StreamTrait}};
use opus::Channels::Mono;
use ringbuf::{HeapRb, traits::{Consumer, Observer, Producer, Split}};
use serde::Serialize;
use sonora::{AudioProcessing, Config, StreamConfig, config::{EchoCanceller, GainController2, NoiseSuppression}};
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

/// Builds the echo canceller/noise suppressor. Both streams are declared as
/// 48 kHz mono, whatever the devices run at: the mic is resampled and downmixed
/// before capture processing, and the mix is mono 48 kHz before render
/// processing. Declaring the device's format instead makes sonora read frames
/// of the wrong size.
fn audio_processing(config: Config) -> AudioProcessing {
    let stream = StreamConfig::new(DESIRED_INPUT_SAMPLE_RATE, 1);
    AudioProcessing::builder()
        .config(config)
        .capture_config(stream)
        .render_config(stream)
        .build()
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

    #[error("Opus error")]
    OpusError(#[from] opus::Error),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
}

pub const DESIRED_INPUT_SAMPLE_RATE: u32 = 48_000;
// pub const DESIRED_OUTPUT_SAMPLE_RATE: u32 = 48_000;

/// 10 ms at 48 kHz: the frame size for microphone input.
const MIC_FRAME_SAMPLES: usize = DESIRED_INPUT_SAMPLE_RATE as usize / 100;

pub struct AudioModule {
    // The latest audio devices! Eventually we can update the list live, but for
    // now it is loaded once at startup.
    input_devices: tokio::sync::watch::Sender<Vec<AudioDevice>>,
    preferred_input_device: tokio::sync::watch::Sender<Option<AudioDevice>>,

    output_devices: tokio::sync::watch::Sender<Vec<AudioDevice>>,
    preferred_output_device: tokio::sync::watch::Sender<Option<AudioDevice>>,
}

pub trait MicFrameSink {
    fn on_mic_frame(&mut self, frame: &[u8], frame_counter: u64, is_terminator: bool);
}

impl<F: FnMut(&[u8], u64, bool)> MicFrameSink for F {
    fn on_mic_frame(&mut self, frame: &[u8], frame_counter: u64, is_terminator: bool) {
        (self)(frame, frame_counter, is_terminator);
    }
}

fn find_device(devices: &[AudioDevice], id: &str) -> Result<AudioDevice, AudioError> {
    devices
        .iter()
        .find(|device| device.id == id)
        .cloned()
        .ok_or_else(|| AudioError::UnknownDevice(id.to_string()))
}

impl AudioModule {
    pub fn new() -> Result<Self, AudioError> {
        let input_devices = get_audio_input_devices()?;
        let output_devices = get_audio_output_devices()?;

        let (input_devices_tx, _) = tokio::sync::watch::channel(input_devices);
        let (output_devices_tx, _) = tokio::sync::watch::channel(output_devices);

        // TODO: load from saved location.
        let (preferred_input_device_tx, _) = tokio::sync::watch::channel(None);
        let (preferred_output_device_tx, _) = tokio::sync::watch::channel(None);

        Ok(AudioModule {
            input_devices: input_devices_tx,
            preferred_input_device: preferred_input_device_tx,
            output_devices: output_devices_tx,
            preferred_output_device: preferred_output_device_tx,
        })
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

    /// Prefers the listed input device with this `id` the next time audio starts.
    pub fn select_input_device(&self, id: &str) -> Result<(), AudioError> {
        let device = find_device(&self.input_devices.borrow(), id)?;
        self.set_preferred_input_device(Some(device));
        Ok(())
    }

    /// Prefers the listed output device with this `id`.
    pub fn select_output_device(&self, id: &str) -> Result<(), AudioError> {
        let device = find_device(&self.output_devices.borrow(), id)?;
        self.set_preferred_output_device(Some(device));
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

    pub fn start(&self, mut on_mic_frame: impl MicFrameSink + 'static, _on_speaker_frame: impl Fn(Vec<f32>) + Send + 'static) -> Result<impl Future<Output = Result<(), AudioError>> + 'static, AudioError> {
        let host = cpal::default_host();
        let preferred_input_device_id = self.preferred_input_device.borrow().as_ref().and_then(|d| DeviceId::from_str(&d.id).ok());

        let input_device = if let Some(id) = preferred_input_device_id && let Some(device) = host.device_by_id(&id) {
            device
        } else {
            host.default_input_device()
                .ok_or_else(|| AudioError::NoAudioDevice)?
        };

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

        let apm_config = Config {
            echo_canceller: Some(EchoCanceller::default()),
            noise_suppression: Some(NoiseSuppression::default()),
            gain_controller2: Some(GainController2::default()),
            ..Default::default()
        };
        let mut apm = audio_processing(apm_config.clone());

        // A best-guess for the network-induced delay.
        // TODO: measure actual network delay and update this value accordingly.
        apm.set_stream_delay_ms(50)?;

        log::info!("Audio input: {:?}, {input_config:?}", input_device.description()?);
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

            let mut encoder = opus::Encoder::new(
                DESIRED_INPUT_SAMPLE_RATE,
                Mono,
                opus::Application::Voip
            )?;

            // Pre-allocate buffers (I think these are on the heap so not huge prob).
            let mut mic_resampler = MicResampler::new(input_rate, DESIRED_INPUT_SAMPLE_RATE)?;
            // Captured mic audio at the device rate, then at 48 kHz awaiting a full frame.
            let mut captured = Vec::new();
            let mut mic_48k = Vec::new();
            let mut output = vec![0u8; 4000];
            let mut frame_counter: u64 = 0;
            let mut speech_detector = SpeechDetector::new(DESIRED_INPUT_SAMPLE_RATE);
            let mut voice_gate = VoiceGate::default();

            let mut mix_interval = tokio::time::interval(std::time::Duration::from_millis(5));
            loop {
                tokio::select! {
                    _ = mix_interval.tick() => {
                        // Send complete mic frames, resampled to 48 kHz first.
                        captured.resize(input_consumer.occupied_len(), 0.0);
                        input_consumer.pop_slice(&mut captured);
                        mic_resampler.process(&captured, &mut mic_48k)?;
                        while mic_48k.len() >= MIC_FRAME_SAMPLES {
                            // Process audio
                            let mut processed = vec![0.0f32; MIC_FRAME_SAMPLES];
                            apm.process_capture_f32(&[&mic_48k[..MIC_FRAME_SAMPLES]], &mut [&mut processed]).map_err(Error::other)?;

                            mic_48k.drain(..MIC_FRAME_SAMPLES);

                            let transmit = voice_gate.update(speech_detector.is_speech(&processed));
                            if transmit == Transmit::Start {
                                // Start each burst fresh rather than from the end of the
                                // previous one, which may have been minutes ago.
                                encoder.reset_state().map_err(Error::other)?;
                            }
                            if transmit != Transmit::Skip {
                                let len = encoder
                                    .encode_float(&processed, &mut output)
                                    .map_err(Error::other)?;
                                let is_terminator = transmit == Transmit::End;
                                // cloned_client.send_audio(&output[..len], frame_counter, is_terminator).map_err(Error::other)?;
                                on_mic_frame.on_mic_frame(&output[..len], frame_counter, is_terminator);
                            }
                            // Opus expects 10ms chunks, we send 10ms. Counts unsent frames
                            // too, so the frame number keeps tracking time across silence.
                            frame_counter += 1;
                        }
                    }
                }
            }

            Ok::<(), AudioError>(())
        };

        Ok(task)
    }
}

/// Converts mono mic audio from the device's sample rate to the 48 kHz Opus
/// expects. Feeding audio at the wrong rate shifts its pitch and speed.
enum MicResampler {
    /// The mic already runs at 48 kHz.
    Passthrough,
    Resample {
        resampler: Box<rubato::Fft<f32>>,
        /// Input not yet resampled: rubato consumes fixed-size chunks.
        pending: Vec<f32>,
        scratch: Vec<f32>,
    },
}

impl MicResampler {
    fn new(input_rate: u32, output_rate: u32) -> Result<Self, Error> {
        if input_rate == output_rate {
            return Ok(Self::Passthrough);
        }
        let chunk = input_rate as usize / 100; // 10 ms of input per step
        let resampler = Box::new(
            rubato::Fft::new(
                input_rate as usize,
                output_rate as usize,
                chunk,
                1,
                rubato::FixedSync::Input,
            )
            .map_err(Error::other)?,
        );
        let scratch = vec![0.0; rubato::Resampler::output_frames_max(&*resampler)];
        Ok(Self::Resample { resampler, pending: Vec::new(), scratch })
    }

    /// Appends the 48 kHz version of `input` to `output`. Input that doesn't
    /// fill a whole chunk yet is kept for the next call.
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), Error> {
        let Self::Resample { resampler, pending, scratch } = self else {
            output.extend_from_slice(input);
            return Ok(());
        };
        use rubato::Resampler;
        use rubato::audioadapter_buffers::direct::InterleavedSlice;

        pending.extend_from_slice(input);
        let mut consumed = 0;
        while pending.len() - consumed >= resampler.input_frames_next() {
            let frames_in = resampler.input_frames_next();
            let frames_out = scratch.len();
            let chunk_in = InterleavedSlice::new(&pending[consumed..consumed + frames_in], 1, frames_in)
                .map_err(Error::other)?;
            let mut chunk_out = InterleavedSlice::new_mut(&mut scratch[..], 1, frames_out).map_err(Error::other)?;
            let (used, written) = resampler
                .process_into_buffer(&chunk_in, &mut chunk_out, None)
                .map_err(Error::other)?;
            output.extend_from_slice(&scratch[..written]);
            consumed += used;
        }
        pending.drain(..consumed);
        Ok(())
    }
}

/// How long to keep sending after speech stops, so pauses between words and
/// quiet word endings aren't cut off: 250 ms of 10 ms mic frames.
const VOICE_HOLD_FRAMES: usize = 25;

/// Detects speech in 10 ms mic frames, using the neural voice detector sonora
/// runs internally for its gain control.
struct SpeechDetector(sonora_agc2::vad_wrapper::VoiceActivityDetectorWrapper);

impl SpeechDetector {
    fn new(sample_rate: u32) -> Self {
        Self(sonora_agc2::vad_wrapper::VoiceActivityDetectorWrapper::new(
            sonora_simd::detect_backend(),
            sample_rate as i32,
        ))
    }

    fn is_speech(&mut self, frame: &[f32]) -> bool {
        // Like WebRTC, sonora's internals scale floats like i16 samples, not to
        // [-1, 1]; unscaled audio reads as silence.
        let scaled: [f32; MIC_FRAME_SAMPLES] = std::array::from_fn(|i| frame[i] * 32768.0);
        self.0.analyze(&scaled) > 0.5
    }
}

/// What to do with a mic frame.
#[derive(Debug, PartialEq)]
enum Transmit {
    /// Not talking: send nothing.
    Skip,
    /// The first frame of a burst.
    Start,
    Continue,
    /// The last frame of a burst, marked as a terminator.
    End,
}

/// Decides which mic frames to send: speech, plus a short hold after it.
#[derive(Default)]
struct VoiceGate {
    /// Frames since speech was last heard, while a burst is in progress.
    frames_since_speech: Option<usize>,
}

impl VoiceGate {
    fn update(&mut self, is_speech: bool) -> Transmit {
        match (self.frames_since_speech, is_speech) {
            (None, false) => Transmit::Skip,
            (None, true) => {
                self.frames_since_speech = Some(0);
                Transmit::Start
            }
            (Some(_), true) => {
                self.frames_since_speech = Some(0);
                Transmit::Continue
            }
            (Some(frames), false) if frames + 1 >= VOICE_HOLD_FRAMES => {
                self.frames_since_speech = None;
                Transmit::End
            }
            (Some(frames), false) => {
                self.frames_since_speech = Some(frames + 1);
                Transmit::Continue
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{sine, tone_strength};
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

    fn device(id: &str) -> AudioDevice {
        AudioDevice { id: id.into(), name: id.into(), is_default: false }
    }

    fn module_with(inputs: Vec<AudioDevice>, outputs: Vec<AudioDevice>) -> AudioModule {
        let module = AudioModule::new().unwrap();
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

        module.select_input_device("b").unwrap();

        assert_eq!(preferred_id(&module.preferred_input_device).as_deref(), Some("b"));
    }

    #[test]
    fn selecting_a_listed_output_device_makes_it_the_preference() {
        let module = module_with(vec![], vec![device("x"), device("y")]);

        module.select_output_device("y").unwrap();

        assert_eq!(preferred_id(&module.preferred_output_device).as_deref(), Some("y"));
        assert_eq!(preferred_id(&module.preferred_input_device), None);
    }

    #[test]
    fn selecting_an_unlisted_device_fails_and_keeps_the_preference() {
        let module = module_with(vec![device("a")], vec![device("x")]);
        module.select_input_device("a").unwrap();

        assert!(matches!(module.select_input_device("nope"), Err(AudioError::UnknownDevice(id)) if id == "nope"));
        assert!(matches!(module.select_output_device("a"), Err(AudioError::UnknownDevice(_))), "inputs are not outputs");

        assert_eq!(preferred_id(&module.preferred_input_device).as_deref(), Some("a"));
        assert_eq!(preferred_id(&module.preferred_output_device), None);
    }

    #[test]
    fn the_preference_can_change_again() {
        let module = module_with(vec![device("a"), device("b")], vec![]);

        module.select_input_device("a").unwrap();
        module.select_input_device("b").unwrap();

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

    /// A mic at 44.1 kHz stereo is resampled/downmixed to 48 kHz mono before
    /// the APM sees it, so the APM must accept 10 ms 48 kHz mono frames.
    #[test]
    fn audio_processing_accepts_48k_mono_frames() {
        let mut apm = audio_processing(Config::default());
        let mut capture_out = vec![0.0f32; MIC_FRAME_SAMPLES];
        apm.process_capture_f32(&[&[0.1f32; MIC_FRAME_SAMPLES]], &mut [&mut capture_out]).unwrap();
        let mut render_out = [0i16; crate::MIX_FRAME_SAMPLES];
        apm.process_render_i16(&[1000i16; crate::MIX_FRAME_SAMPLES], &mut render_out).unwrap();
    }


    #[test]
    fn mic_audio_at_48_khz_passes_through_unchanged() {
        let mut resampler = MicResampler::new(DESIRED_INPUT_SAMPLE_RATE, DESIRED_INPUT_SAMPLE_RATE).unwrap();
        let input = sine(440.0, DESIRED_INPUT_SAMPLE_RATE, 4800);

        let mut output = Vec::new();
        resampler.process(&input, &mut output).unwrap();

        assert_eq!(output, input);
    }

    #[test]
    fn mic_audio_at_44_1_khz_is_resampled_without_changing_pitch() {
        let mut resampler = MicResampler::new(44_100, DESIRED_INPUT_SAMPLE_RATE).unwrap();
        // One second, fed in uneven pieces like a real capture callback.
        let input = sine(440.0, 44_100, 44_100);
        let mut output = Vec::new();
        for chunk in input.chunks(337) {
            resampler.process(chunk, &mut output).unwrap();
        }

        // About one second at 48 kHz: the clock is preserved…
        assert!((47_000..=48_000).contains(&output.len()), "got {} samples", output.len());
        // …and so is the pitch. Treating 44.1 kHz audio as 48 kHz without
        // resampling would shift 440 Hz up to ~479 Hz (the "mousy" bug).
        let settled = &output[4800..];   // skip the resampler's startup delay
        assert!(tone_strength(settled, 440.0) > 0.4, "440 Hz missing");
        assert!(tone_strength(settled, 479.0) < 0.05, "pitch shifted up");
    }


    /// 10 ms frames of a speech-like sound: a 140 Hz voice with harmonics,
    /// pulsing at a syllable-ish 4 Hz.
    fn voiced_frames(frames: usize) -> Vec<Vec<f32>> {
        (0..frames)
            .map(|frame| {
                (0..MIC_FRAME_SAMPLES)
                    .map(|i| {
                        let t = (frame * MIC_FRAME_SAMPLES + i) as f32 / DESIRED_INPUT_SAMPLE_RATE as f32;
                        let envelope = 0.5 + 0.5 * (t * 4.0 * std::f32::consts::TAU).sin();
                        let voice: f32 = (1..25)
                            .map(|k| (t * 140.0 * k as f32 * std::f32::consts::TAU).sin() / k as f32)
                            .sum();
                        0.15 * envelope * voice
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn speech_detector_hears_a_voice() {
        let mut detector = SpeechDetector::new(DESIRED_INPUT_SAMPLE_RATE);

        let speech = voiced_frames(10).iter().filter(|frame| detector.is_speech(frame)).count();

        // The detector needs a frame or two to react.
        assert!(speech >= 8, "only {speech} of 10 voiced frames were speech");
    }

    #[test]
    fn speech_detector_ignores_silence() {
        let mut detector = SpeechDetector::new(DESIRED_INPUT_SAMPLE_RATE);

        let speech = (0..10).filter(|_| detector.is_speech(&[0.0; MIC_FRAME_SAMPLES])).count();

        assert_eq!(speech, 0);
    }

    #[test]
    fn voice_gate_sends_nothing_while_silent() {
        let mut gate = VoiceGate::default();

        assert_eq!(gate.update(false), Transmit::Skip);
        assert_eq!(gate.update(false), Transmit::Skip);
    }

    #[test]
    fn voice_gate_bridges_pauses_shorter_than_the_hold() {
        let mut gate = VoiceGate::default();

        assert_eq!(gate.update(true), Transmit::Start);
        for _ in 0..VOICE_HOLD_FRAMES - 1 {
            assert_eq!(gate.update(false), Transmit::Continue);
        }
        assert_eq!(gate.update(true), Transmit::Continue, "the pause should not end the burst");
    }

    #[test]
    fn voice_gate_ends_the_burst_after_the_hold() {
        let mut gate = VoiceGate::default();

        gate.update(true);
        let after: Vec<Transmit> = (0..VOICE_HOLD_FRAMES + 1).map(|_| gate.update(false)).collect();

        assert!(after[..VOICE_HOLD_FRAMES - 1].iter().all(|t| *t == Transmit::Continue), "{after:?}");
        assert_eq!(after[VOICE_HOLD_FRAMES - 1..], [Transmit::End, Transmit::Skip]);
        assert_eq!(gate.update(true), Transmit::Start, "new speech starts a new burst");
    }
}
