use std::io::Error;

use opus::Channels::Mono;
use sonora::{AudioProcessing, Config, StreamConfig, config::{AdaptiveDigital, EchoCanceller, GainController2}};

use crate::PipelineError;

pub const DESIRED_INPUT_SAMPLE_RATE: u32 = 48_000;

/// 10 ms at 48 kHz: the frame size for microphone input.
pub const MIC_FRAME_SAMPLES: usize = DESIRED_INPUT_SAMPLE_RATE as usize / 100;

/// What the mic is cleaned up with before it is sent.
pub fn apm_config() -> Config {
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
pub fn audio_processing(config: Config) -> AudioProcessing {
    let stream = StreamConfig::new(DESIRED_INPUT_SAMPLE_RATE, 1);
    AudioProcessing::builder()
        .config(config)
        .capture_config(stream)
        .render_config(stream)
        .build()
}

pub trait MicFrameSink: Send {
    fn on_mic_frame(&mut self, frame: &[u8], frame_counter: u64, is_terminator: bool);
}

impl<F: FnMut(&[u8], u64, bool) + Send> MicFrameSink for F {
    fn on_mic_frame(&mut self, frame: &[u8], frame_counter: u64, is_terminator: bool) {
        (self)(frame, frame_counter, is_terminator);
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

/// Everything between raw mic samples and the Opus packets sent to the server:
/// resampling, echo cancellation, denoising, speech gating and encoding.
pub struct MicPipeline {
    resampler: MicResampler,
    /// Mic audio at 48 kHz, awaiting a full 10 ms frame.
    mic_48k: Vec<f32>,
    encoder: opus::Encoder,
    output: Vec<u8>,
    frame_counter: u64,
    denoiser: Denoiser,
    speech_detector: SpeechDetector,
    voice_gate: VoiceGate,
}

impl MicPipeline {
    /// `input_rate` is the mic device's sample rate; samples are mono.
    pub fn new(input_rate: u32) -> Result<Self, PipelineError> {
        Ok(Self {
            resampler: MicResampler::new(input_rate, DESIRED_INPUT_SAMPLE_RATE)?,
            mic_48k: Vec::new(),
            encoder: opus::Encoder::new(DESIRED_INPUT_SAMPLE_RATE, Mono, opus::Application::Voip)?,
            output: vec![0u8; 4000],
            frame_counter: 0,
            denoiser: Denoiser::new(),
            speech_detector: SpeechDetector::new(DESIRED_INPUT_SAMPLE_RATE),
            voice_gate: VoiceGate::default(),
        })
    }

    /// Processes newly captured mono samples, handing every packet that comes
    /// out (at most one per complete 10 ms) to `sink`. `apm` is shared with
    /// playback, which feeds it the echo canceller's reference signal.
    pub fn process(
        &mut self,
        captured: &[f32],
        apm: &mut AudioProcessing,
        noise_suppression: bool,
        sink: &mut impl MicFrameSink,
    ) -> Result<(), PipelineError> {
        self.resampler.process(captured, &mut self.mic_48k)?;
        while self.mic_48k.len() >= MIC_FRAME_SAMPLES {
            let mut processed = [0.0f32; MIC_FRAME_SAMPLES];
            apm.process_capture_f32(&[&self.mic_48k[..MIC_FRAME_SAMPLES]], &mut [&mut processed])?;
            self.mic_48k.drain(..MIC_FRAME_SAMPLES);

            // After echo cancellation, which needs the mic signal to stay linear.
            if noise_suppression {
                self.denoiser.process(&mut processed);
            }

            let transmit = self.voice_gate.update(self.speech_detector.is_speech(&processed));
            if transmit == Transmit::Start {
                // Start each burst fresh rather than from the end of the
                // previous one, which may have been minutes ago.
                self.encoder.reset_state()?;
            }
            if transmit != Transmit::Skip {
                let len = self.encoder.encode_float(&processed, &mut self.output)?;
                let is_terminator = transmit == Transmit::End;
                sink.on_mic_frame(&self.output[..len], self.frame_counter, is_terminator);
            }
            // Opus expects 10ms chunks, we send 10ms. Counts unsent frames
            // too, so the frame number keeps tracking time across silence.
            self.frame_counter += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{sine, tone_strength};

    /// A mic at 44.1 kHz stereo is resampled/downmixed to 48 kHz mono before
    /// the APM sees it, so the APM must accept 10 ms 48 kHz mono frames.
    #[test]
    fn audio_processing_accepts_48k_mono_frames() {
        let mut apm = audio_processing(Config::default());
        let mut capture_out = vec![0.0f32; MIC_FRAME_SAMPLES];
        apm.process_capture_f32(&[&[0.1f32; MIC_FRAME_SAMPLES]], &mut [&mut capture_out]).unwrap();
        let mut render_out = [0i16; crate::speaker::SPEAKER_FRAME_SAMPLES];
        apm.process_render_i16(&[1000i16; crate::speaker::SPEAKER_FRAME_SAMPLES], &mut render_out).unwrap();
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
