use std::{io::Error, str::FromStr};

use cpal::{DeviceId, traits::{DeviceTrait, HostTrait, StreamTrait}};
use opus::Channels::Mono;
use ringbuf::{HeapRb, traits::{Consumer, Observer, Producer, Split}};
use serde::{Deserialize, Serialize};
use sonora::{AudioProcessing, Config, StreamConfig, config::{AdaptiveDigital, EchoCanceller, GainController2}};
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

/// What the mic is cleaned up with before it is sent.
fn apm_config() -> Config {
    Config {
        echo_canceller: Some(EchoCanceller::default()),
        // No sonora noise suppression: RNNoise (`Denoiser`) does it after echo cancellation.
        // `GainController2::default()` has no adaptive controller and so does nothing but
        // limit; the adaptive digital gain is what actually raises a quiet mic.
        gain_controller2: Some(GainController2 {
            adaptive_digital: Some(AdaptiveDigital::default()),
            ..Default::default()
        }),
        ..Default::default()
    }
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

/// Speakers are mixed and played at 48 kHz mono, whatever the output device runs at.
pub const SPEAKER_SAMPLE_RATE: u32 = 48_000;

/// 10 ms at 48 kHz: the frame size a `SpeakerSource` produces.
pub const SPEAKER_FRAME_SAMPLES: usize = SPEAKER_SAMPLE_RATE as usize / 100;

/// Room for ~200 ms of queued playback.
const SPEAKER_HEADROOM_SAMPLES: usize = SPEAKER_SAMPLE_RATE as usize / 1000 * 200;

/// Playback is topped up to this level each tick. NetEq absorbs network jitter,
/// so this only needs to cover timer slop.
const SPEAKER_TARGET_SAMPLES: usize = SPEAKER_SAMPLE_RATE as usize / 1000 * 40;

/// What the module plays: the mixed audio of everyone who is talking.
pub trait SpeakerSource: Send {
    /// The next 10 ms to play. `None` means there will be no more (e.g. the
    /// connection closed), which stops the module.
    fn next_frame(&mut self) -> Result<Option<[i16; SPEAKER_FRAME_SAMPLES]>, Error>;
}

impl<F> SpeakerSource for F
where
    F: FnMut() -> Result<Option<[i16; SPEAKER_FRAME_SAMPLES]>, Error> + Send,
{
    fn next_frame(&mut self) -> Result<Option<[i16; SPEAKER_FRAME_SAMPLES]>, Error> {
        self()
    }
}

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

/// Tops `output` up to `target_samples` from `speakers`. Everything played also
/// goes to the echo canceller as its reference, so the mic can subtract it.
/// Returns `false` once the source has ended.
fn top_up_output(
    output: &mut impl Producer<Item = i16>,
    target_samples: usize,
    speakers: &mut impl SpeakerSource,
    apm: &mut AudioProcessing,
) -> Result<bool, AudioError> {
    while output.occupied_len() < target_samples {
        let Some(frame) = speakers.next_frame()? else {
            return Ok(false);
        };
        let mut played = [0i16; SPEAKER_FRAME_SAMPLES];
        apm.process_render_i16(&frame, &mut played)?;
        output.push_slice(&played);
    }
    Ok(true)
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

pub trait MicFrameSink: Send {
    fn on_mic_frame(&mut self, frame: &[u8], frame_counter: u64, is_terminator: bool);
}

impl<F: FnMut(&[u8], u64, bool) + Send> MicFrameSink for F {
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
            let mut denoiser = Denoiser::new();
            let mut speech_detector = SpeechDetector::new(DESIRED_INPUT_SAMPLE_RATE);
            let mut voice_gate = VoiceGate::default();

            let mut mix_interval = tokio::time::interval(std::time::Duration::from_millis(5));
            loop {
                tokio::select! {
                    _ = mix_interval.tick() => {
                        // Top the ring buffer up to a fixed level rather than mixing once per
                        // tick: the card drains it at its own rate, so this follows the card's
                        // clock instead of drifting against this timer.
                        if !top_up_output(&mut output_producer, SPEAKER_TARGET_SAMPLES, &mut speakers, &mut apm)? {
                            log::info!("Nothing left to play; stopping audio");
                            break;
                        }

                        // Send complete mic frames, resampled to 48 kHz first.
                        captured.resize(input_consumer.occupied_len(), 0.0);
                        input_consumer.pop_slice(&mut captured);
                        mic_resampler.process(&captured, &mut mic_48k)?;
                        while mic_48k.len() >= MIC_FRAME_SAMPLES {
                            // Process audio
                            let mut processed = [0.0f32; MIC_FRAME_SAMPLES];
                            apm.process_capture_f32(&[&mic_48k[..MIC_FRAME_SAMPLES]], &mut [&mut processed]).map_err(Error::other)?;

                            mic_48k.drain(..MIC_FRAME_SAMPLES);

                            // After echo cancellation, which needs the mic signal to stay linear.
                            if *noise_suppression.borrow() {
                                denoiser.process(&mut processed);
                            }

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

/// Suppresses background noise in 10 ms mic frames with RNNoise.
struct Denoiser(Box<nnnoiseless::DenoiseState<'static>>);

impl Denoiser {
    fn new() -> Self {
        Self(nnnoiseless::DenoiseState::new())
    }

    fn process(&mut self, frame: &mut [f32; MIC_FRAME_SAMPLES]) {
        // RNNoise works on 16-bit-scaled samples; one frame is exactly 10 ms at 48 kHz.
        let scaled: [f32; MIC_FRAME_SAMPLES] = std::array::from_fn(|i| frame[i] * 32768.0);
        self.0.process_frame(frame, &scaled);
        for sample in frame.iter_mut() {
            *sample /= 32768.0;
        }
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

    type Frame = [i16; SPEAKER_FRAME_SAMPLES];

    fn ring(capacity: usize) -> (ringbuf::HeapProd<i16>, ringbuf::HeapCons<i16>) {
        HeapRb::<i16>::new(capacity).split()
    }

    #[test]
    fn top_up_fills_the_ring_to_the_target_with_whole_frames() {
        let (mut producer, mut consumer) = ring(4800);
        let mut apm = audio_processing(Config::default());
        let mut speakers = || Ok(Some([1000i16; SPEAKER_FRAME_SAMPLES]));

        let more = top_up_output(&mut producer, 1000, &mut speakers, &mut apm).unwrap();

        assert!(more);
        // 1000 isn't a whole number of 480-sample frames: it stops at the first level above.
        assert_eq!(consumer.occupied_len(), 3 * SPEAKER_FRAME_SAMPLES);
        let mut played = vec![0i16; consumer.occupied_len()];
        consumer.pop_slice(&mut played);
        assert!(played.iter().all(|&s| s == 1000), "the speakers' audio should pass through unchanged");
    }

    #[test]
    fn top_up_does_nothing_when_the_ring_is_already_full_enough() {
        let (mut producer, consumer) = ring(4800);
        producer.push_slice(&[0i16; 2000]);
        let mut apm = audio_processing(Config::default());
        let mut asked = 0;
        let mut speakers = || {
            asked += 1;
            Ok(Some([0i16; SPEAKER_FRAME_SAMPLES]))
        };

        assert!(top_up_output(&mut producer, 1000, &mut speakers, &mut apm).unwrap());

        assert_eq!(asked, 0);
        assert_eq!(consumer.occupied_len(), 2000);
    }

    #[test]
    fn top_up_reports_when_the_speakers_have_ended() {
        let (mut producer, consumer) = ring(4800);
        let mut apm = audio_processing(Config::default());
        let mut frames_left = 2;
        let mut speakers = || {
            Ok(if frames_left > 0 {
                frames_left -= 1;
                Some([5i16; SPEAKER_FRAME_SAMPLES])
            } else {
                None
            })
        };

        let more = top_up_output(&mut producer, 4000, &mut speakers, &mut apm).unwrap();

        assert!(!more);
        assert_eq!(consumer.occupied_len(), 2 * SPEAKER_FRAME_SAMPLES, "frames before the end are still played");
    }

    #[test]
    fn top_up_passes_source_errors_on() {
        let (mut producer, _consumer) = ring(4800);
        let mut apm = audio_processing(Config::default());
        let mut speakers = || -> Result<Option<Frame>, Error> { Err(Error::other("decoder broke")) };

        let result = top_up_output(&mut producer, 1000, &mut speakers, &mut apm);

        assert!(matches!(result, Err(AudioError::IoError(_))));
    }

    /// Plays noise from "the other end" and models the room as a delayed,
    /// quieter copy of it reaching the mic. Returns how much of that echo is
    /// left in the processed mic audio over the last second (1.0 = all of it).
    fn echo_remaining(feed_render_through_top_up: bool) -> f32 {
        const TICKS: usize = 800; // 8 s: the canceller needs a while to adapt
        const MEASURED_TICKS: usize = 100;
        const ROOM_DELAY_FRAMES: usize = 5; // 50 ms from ring to mic
        const ECHO_GAIN: f32 = 0.5;
        let energy = |frame: &[f32]| frame.iter().map(|s| s * s).sum::<f32>();

        let mut apm = audio_processing(Config {
            echo_canceller: Some(EchoCanceller::default()),
            ..Default::default()
        });
        apm.set_stream_delay_ms(50).unwrap();

        let mut seed = 12345u32;
        let mut far_end = move || {
            Ok(Some(std::array::from_fn::<i16, SPEAKER_FRAME_SAMPLES, _>(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 16) as i16) / 8 // white noise, about 12% of full scale
            })))
        };

        let (mut producer, mut consumer) = ring(4800);
        let mut room = std::collections::VecDeque::from(vec![[0.0f32; SPEAKER_FRAME_SAMPLES]; ROOM_DELAY_FRAMES]);
        let (mut echo, mut left) = (0.0, 0.0);
        for tick in 0..TICKS {
            if feed_render_through_top_up {
                top_up_output(&mut producer, SPEAKER_FRAME_SAMPLES, &mut far_end, &mut apm).unwrap();
            } else {
                let frame: Frame = far_end().unwrap().unwrap();
                producer.push_slice(&frame);
            }

            // The sound card plays one frame...
            let mut played = [0i16; SPEAKER_FRAME_SAMPLES];
            consumer.pop_slice(&mut played);
            // ...which reaches the mic a little later and quieter.
            room.push_back(played.map(|s| s as f32 / i16::MAX as f32 * ECHO_GAIN));
            let mic = room.pop_front().unwrap();

            let mut processed = [0.0f32; SPEAKER_FRAME_SAMPLES];
            apm.process_capture_f32(&[&mic], &mut [&mut processed]).unwrap();
            if tick >= TICKS - MEASURED_TICKS {
                echo += energy(&mic);
                left += energy(&processed);
            }
        }
        left / echo
    }

    #[test]
    fn the_echo_canceller_removes_what_the_speakers_played() {
        let remaining = echo_remaining(true);
        eprintln!("echo remaining with the render reference: {remaining}");
        assert!(remaining < 0.01, "{remaining} of the echo is still in the mic");
    }

    #[test]
    fn without_the_render_reference_the_echo_stays() {
        // Guards the test above: it only means something if cancelling needs the reference.
        let remaining = echo_remaining(false);
        eprintln!("echo remaining without the render reference: {remaining}");
        assert!(remaining > 0.2, "the echo went away without a reference: {remaining}");
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

    /// The voice, scaled by `gain`, through the mic processing for 8 s. Returns
    /// the RMS level of the input and of the output over the last second, and
    /// the loudest output sample.
    fn voice_through_the_mic_processing(gain: f32) -> (f32, f32, f32) {
        let mut apm = audio_processing(apm_config());
        let (mut input_energy, mut output_energy, mut peak, mut samples) = (0.0f32, 0.0f32, 0.0f32, 0usize);
        for (n, frame) in voiced_frames(800).iter().enumerate() {
            let input: Vec<f32> = frame.iter().map(|s| s * gain).collect();
            let mut output = vec![0.0f32; MIC_FRAME_SAMPLES];
            apm.process_capture_f32(&[&input], &mut [&mut output]).unwrap();
            if n >= 700 {
                input_energy += input.iter().map(|s| s * s).sum::<f32>();
                output_energy += output.iter().map(|s| s * s).sum::<f32>();
                peak = output.iter().fold(peak, |peak, s| peak.max(s.abs()));
                samples += input.len();
            }
        }
        ((input_energy / samples as f32).sqrt(), (output_energy / samples as f32).sqrt(), peak)
    }

    #[test]
    fn a_quiet_voice_is_brought_up_to_a_normal_level() {
        let (heard, sent, _) = voice_through_the_mic_processing(0.1);
        eprintln!("quiet voice: in {heard}, out {sent}, boost {:.1} dB", 20.0 * (sent / heard).log10());
        assert!(sent > 2.0 * heard, "a quiet voice was sent at {sent} from {heard}: barely louder");
    }

    #[test]
    fn a_loud_voice_is_not_pushed_into_clipping() {
        let (_, _, peak) = voice_through_the_mic_processing(4.0);
        assert!(peak <= 1.0, "peaked at {peak}");
    }

    #[test]
    fn quiet_background_noise_is_not_boosted_into_something_audible() {
        let mut apm = audio_processing(apm_config());
        let mut seed = 99u32;
        let mut energy = 0.0f32;
        for n in 0..800 {
            // White noise at about -60 dBFS: a quiet room.
            let input: Vec<f32> = (0..MIC_FRAME_SAMPLES)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((seed >> 16) as i16 as f32 / 32768.0) * 0.002
                })
                .collect();
            let mut output = vec![0.0f32; MIC_FRAME_SAMPLES];
            apm.process_capture_f32(&[&input], &mut [&mut output]).unwrap();
            if n >= 700 {
                energy += output.iter().map(|s| s * s).sum::<f32>();
            }
        }
        let rms = (energy / (100 * MIC_FRAME_SAMPLES) as f32).sqrt();
        eprintln!("room noise out: {rms} ({:.1} dBFS)", 20.0 * rms.log10());
        assert!(rms < 0.005, "room noise came out at {rms}");
    }

    #[test]
    fn speech_detector_hears_a_voice() {
        let mut detector = SpeechDetector::new(DESIRED_INPUT_SAMPLE_RATE);

        let speech = voiced_frames(10).iter().filter(|frame| detector.is_speech(frame)).count();

        // The detector needs a frame or two to react.
        assert!(speech >= 8, "only {speech} of 10 voiced frames were speech");
    }

    #[test]
    fn denoiser_keeps_frames_at_unit_scale() {
        let mut denoiser = Denoiser::new();
        let mut frame = [0.0f32; MIC_FRAME_SAMPLES];
        for _ in 0..20 {
            frame = std::array::from_fn(|i| 0.3 * (i as f32 * 0.1).sin());
            denoiser.process(&mut frame);
        }
        assert!(frame.iter().all(|s| s.abs() <= 1.0));
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
