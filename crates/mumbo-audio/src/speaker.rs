use std::collections::HashMap;
use std::io::Error;

use mumble_protocol as proto;
use ringbuf::traits::Producer;
use sonora::AudioProcessing;

use crate::PipelineError;

/// Speakers are mixed and played at 48 kHz mono, whatever the output device runs at.
pub const SPEAKER_SAMPLE_RATE: u32 = 48_000;

/// 10 ms at 48 kHz: the frame size a `SpeakerSource` produces.
pub const SPEAKER_FRAME_SAMPLES: usize = SPEAKER_SAMPLE_RATE as usize / 100;

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

/// Tops `output` up to `target_samples` from `speakers`. Everything played also
/// goes to the echo canceller as its reference, so the mic can subtract it.
/// Returns `false` once the source has ended.
pub fn top_up_output(
    output: &mut impl Producer<Item = i16>,
    target_samples: usize,
    speakers: &mut impl SpeakerSource,
    apm: &mut AudioProcessing,
) -> Result<bool, PipelineError> {
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

const OUTPUT_SAMPLE_RATE: u32 = SPEAKER_SAMPLE_RATE;

/// 10 ms at 48 kHz: the frame size `NetEq::get_audio` produces.
const MIX_FRAME_SAMPLES: usize = SPEAKER_FRAME_SAMPLES;

/// NetEq routes packets to decoders by RTP payload type; we only have Opus.
const OPUS_PAYLOAD_TYPE: u8 = 0;

/// Adapts libopus to NetEq's decoder interface.
struct OpusDecoder(opus::Decoder);

impl neteq::codec::AudioDecoder for OpusDecoder {
    fn sample_rate(&self) -> u32 {
        OUTPUT_SAMPLE_RATE
    }

    fn channels(&self) -> u8 {
        1
    }

    fn decode(&mut self, encoded: &[u8]) -> neteq::Result<Vec<f32>> {
        // 120 ms, Opus's largest frame.
        let mut pcm = vec![0.0; OUTPUT_SAMPLE_RATE as usize * 120 / 1000];
        let count = self
            .0
            .decode_float(encoded, &mut pcm, false)
            .map_err(|e| neteq::NetEqError::DecoderError(e.to_string()))?;
        pcm.truncate(count);
        Ok(pcm)
    }
}

/// One remote talker: a jitter buffer that decodes their Opus packets.
struct Speaker {
    neteq: neteq::NetEq,
    // Over the TCP tunnel packets arrive complete and in order, so a running
    // count is an exact sequence/timestamp. UDP will need Mumble's frame_number.
    next_sequence: u16,
    next_timestamp: u32,
}

impl Speaker {
    fn new() -> Result<Self, Error> {
        let mut neteq = neteq::NetEq::new(neteq::NetEqConfig {
            sample_rate: OUTPUT_SAMPLE_RATE,
            channels: 1,
            ..Default::default()
        })
        .map_err(Error::other)?;
        let decoder =
            opus::Decoder::new(OUTPUT_SAMPLE_RATE, opus::Channels::Mono).map_err(Error::other)?;
        neteq.register_decoder(OPUS_PAYLOAD_TYPE, Box::new(OpusDecoder(decoder)));
        Ok(Self {
            neteq,
            next_sequence: 0,
            next_timestamp: 0,
        })
    }

    fn insert_packet(&mut self, session: u32, opus_data: &[u8]) -> Result<(), Error> {
        let samples =
            opus::packet::get_nb_samples(opus_data, OUTPUT_SAMPLE_RATE).map_err(Error::other)?;
        let header = neteq::RtpHeader::new(
            self.next_sequence,
            self.next_timestamp,
            session,
            OPUS_PAYLOAD_TYPE,
            false,
        );
        let duration_ms = (samples * 1000 / OUTPUT_SAMPLE_RATE as usize) as u32;
        let packet = neteq::AudioPacket::new(
            header,
            opus_data.to_vec(),
            OUTPUT_SAMPLE_RATE,
            1,
            duration_ms,
        );
        self.neteq.insert_packet(packet).map_err(Error::other)?;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.next_timestamp = self.next_timestamp.wrapping_add(samples as u32);
        Ok(())
    }
}

/// Decodes the packets arriving from the server and mixes everyone talking.
pub struct Mixer {
    packets: tokio::sync::mpsc::Receiver<proto::raw_udp::Audio>,
    speakers: HashMap<u32, Speaker>,
}

impl Mixer {
    pub fn new(packets: tokio::sync::mpsc::Receiver<proto::raw_udp::Audio>) -> Self {
        Self { packets, speakers: HashMap::new() }
    }
}

impl SpeakerSource for Mixer {
    fn next_frame(&mut self) -> Result<Option<[i16; MIX_FRAME_SAMPLES]>, Error> {
        loop {
            match self.packets.try_recv() {
                Ok(packet) => {
                    // TODO: volume adjustment
                    let sender = packet.sender_session;
                    let speaker = match self.speakers.entry(sender) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => entry.insert(Speaker::new()?),
                    };
                    if let Err(e) = speaker.insert_packet(sender, &packet.opus_data) {
                        log::warn!("Dropping audio from session {sender}: {e}");
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                // Only once every queued packet has been taken: we disconnected.
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Ok(None),
            }
        }
        mix_frame(&mut self.speakers).map(Some)
    }
}

/// Mixes 10 ms from every speaker who has audio buffered.
fn mix_frame(speakers: &mut HashMap<u32, Speaker>) -> Result<[i16; MIX_FRAME_SAMPLES], Error> {
    let mut sum = [0.0f32; MIX_FRAME_SAMPLES];
    for speaker in speakers.values_mut() {
        // Skip idle speakers so NetEq doesn't synthesize concealment for silence.
        if speaker.neteq.current_buffer_size_samples() == 0 {
            continue;
        }
        let audio = speaker.neteq.get_audio().map_err(Error::other)?;
        for (out, sample) in sum.iter_mut().zip(audio.samples) {
            *out += sample;
        }
    }
    Ok(sum.map(|sample| (soft_clip(sample) * i16::MAX as f32) as i16))
}

/// Passes normal levels through untouched and smoothly bends anything louder
/// (e.g. overlapping speakers) toward, but never past, full scale.
fn soft_clip(sample: f32) -> f32 {
    const THRESHOLD: f32 = 0.8;
    let magnitude = sample.abs();
    if magnitude <= THRESHOLD {
        return sample;
    }
    // tanh's slope is 1 at 0, so the curve continues the straight line at
    // THRESHOLD without a kink, and approaches 1.0 asymptotically.
    let headroom = 1.0 - THRESHOLD;
    let bent = THRESHOLD + headroom * ((magnitude - THRESHOLD) / headroom).tanh();
    bent.copysign(sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mic::audio_processing;
    use crate::test_util::tone_strength;
    use ringbuf::{HeapRb, traits::{Consumer, Observer, Split}};
    use sonora::{Config, config::EchoCanceller};

    /// Opus-encodes `packets` × 20 ms of a sine tone, as a sender would.
    fn tone_packets(frequency: f32, amplitude: f32, packets: usize) -> Vec<Vec<u8>> {
        const PACKET_SAMPLES: usize = 960;
        let mut encoder = opus::Encoder::new(
            OUTPUT_SAMPLE_RATE,
            opus::Channels::Mono,
            opus::Application::Voip,
        )
        .unwrap();
        (0..packets)
            .map(|packet| {
                let pcm: Vec<f32> = (0..PACKET_SAMPLES)
                    .map(|i| {
                        let t = (packet * PACKET_SAMPLES + i) as f32 / OUTPUT_SAMPLE_RATE as f32;
                        (t * frequency * std::f32::consts::TAU).sin() * amplitude
                    })
                    .collect();
                let mut encoded = [0u8; 4000];
                let len = encoder.encode_float(&pcm, &mut encoded).unwrap();
                encoded[..len].to_vec()
            })
            .collect()
    }

    fn energy(frame: &[i16]) -> f32 {
        frame
            .iter()
            .map(|&s| (s as f32 / i16::MAX as f32).powi(2))
            .sum()
    }

    #[test]
    fn speaker_audio_comes_out_of_the_mix() {
        let mut speakers = HashMap::from([(7, Speaker::new().unwrap())]);
        for packet in tone_packets(440.0, 0.5, 25) {
            speakers
                .get_mut(&7)
                .unwrap()
                .insert_packet(7, &packet)
                .unwrap();
        }

        let total: f32 = (0..50)
            .map(|_| energy(&mix_frame(&mut speakers).unwrap()))
            .sum();

        // A 0.5-amplitude tone has mean power 0.125 per sample; 500 ms of it is
        // ~3000. Require a healthy fraction to allow for NetEq's startup delay.
        assert!(total > 500.0, "mix was nearly silent: energy {total}");
    }

    #[test]
    fn two_speakers_are_heard_together() {
        let mut speakers =
            HashMap::from([(1, Speaker::new().unwrap()), (2, Speaker::new().unwrap())]);
        // Quiet enough that the sum stays below soft_clip's threshold.
        let alice = tone_packets(440.0, 0.25, 25);
        let bob = tone_packets(1000.0, 0.25, 25);
        for (alice_packet, bob_packet) in alice.iter().zip(&bob) {
            speakers
                .get_mut(&1)
                .unwrap()
                .insert_packet(1, alice_packet)
                .unwrap();
            speakers
                .get_mut(&2)
                .unwrap()
                .insert_packet(2, bob_packet)
                .unwrap();
        }

        let mixed: Vec<f32> = (0..50)
            .flat_map(|_| mix_frame(&mut speakers).unwrap())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect();

        let (alice_level, bob_level) =
            (tone_strength(&mixed, 440.0), tone_strength(&mixed, 1000.0));
        assert!(alice_level > 0.1, "440 Hz missing from mix: {alice_level}");
        assert!(bob_level > 0.1, "1000 Hz missing from mix: {bob_level}");
        // An unrelated frequency stays near zero, so the levels above are the tones, not noise.
        assert!(tone_strength(&mixed, 3000.0) < 0.02);
    }

    fn audio_packet(session: u32, opus_data: Vec<u8>) -> proto::raw_udp::Audio {
        proto::raw_udp::Audio { sender_session: session, opus_data, ..Default::default() }
    }

    #[test]
    fn mixer_plays_the_packets_it_receives() {
        use crate::speaker::SpeakerSource;
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let mut mixer = Mixer::new(receiver);
        for packet in tone_packets(440.0, 0.5, 25) {
            sender.try_send(audio_packet(7, packet)).unwrap();
        }

        let total: f32 = (0..50)
            .map(|_| energy(&mixer.next_frame().unwrap().expect("still connected")))
            .sum();

        assert!(total > 500.0, "mix was nearly silent: energy {total}");
    }

    #[test]
    fn mixer_ends_when_the_connection_closes() {
        use crate::speaker::SpeakerSource;
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let mut mixer = Mixer::new(receiver);

        let idle = mixer.next_frame().unwrap().expect("connected, just quiet");
        assert_eq!(idle, [0; MIX_FRAME_SAMPLES]);

        drop(sender);
        assert!(mixer.next_frame().unwrap().is_none());
    }

    /// Needs the dev server from dev/mumble-server (`npm run mumble:up`), on
    /// localhost:64738 with password "test". Run with `cargo test -- --ignored`.
    /// Two clients join the root channel; one talks and the other's mixer should
    /// produce the tone, exercising the network, decoding and mixing together.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the dev Mumble server (dev/mumble-server) on localhost:64738"]
    async fn audio_sent_by_one_client_is_mixed_for_another() {
        use crate::speaker::SpeakerSource;
        let connect = |role: &str| {
            proto::client::connect(proto::client::ConnectionOptions {
                server: "localhost".into(),
                port: Some(64738),
                password: Some("test".into()),
                // Unique names: a duplicate name kicks the earlier session.
                username: Some(format!("mumbo-e2e-{role}-{}", std::process::id())),
            })
        };
        let listener = connect("listener").await.expect("connecting the listener");
        let talker = connect("talker").await.expect("connecting the talker");

        let talking = {
            let client = talker.client.clone();
            tokio::spawn(async move {
                let packets = tone_packets(440.0, 0.5, 50); // one second
                for (i, packet) in packets.iter().enumerate() {
                    // Frame numbers count 10 ms; a packet is 20 ms.
                    client.send_audio(packet, i as u64 * 2, i == packets.len() - 1).unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
        };

        let mut mixer = Mixer::new(listener.audio);
        let mut mixed = Vec::new();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(10));
        for _ in 0..250 {
            tick.tick().await;
            let frame = mixer.next_frame().unwrap().expect("the listener stayed connected");
            mixed.extend(frame.map(|s| s as f32 / i16::MAX as f32));
        }
        talking.await.unwrap();
        listener.client.disconnect();
        talker.client.disconnect();

        // Judge 100 ms at a time. The level is checked, not the pitch: NetEq's time
        // stretching bends a pure tone when audio arrives in real time (see TODO.md).
        let windows: Vec<(f32, f32)> = mixed
            .as_chunks::<{ MIX_FRAME_SAMPLES * 10 }>()
            .0
            .iter()
            .map(|w| {
                let rms = (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt();
                (rms, tone_strength(w, 3000.0))
            })
            .collect();
        // A 0.5-amplitude sine has an RMS of about 0.35.
        let loud = windows.iter().filter(|(rms, _)| *rms > 0.25).count();
        assert!(loud >= 5, "the talker was heard in only {loud} of {} windows: {windows:.3?}", windows.len());
        assert!(windows.iter().all(|(_, other)| *other < 0.02), "unrelated frequency in the mix");
    }

    #[test]
    fn soft_clip_leaves_normal_levels_untouched() {
        for sample in [0.0, 0.1, -0.5, 0.79, -0.79] {
            assert_eq!(soft_clip(sample), sample);
        }
    }

    #[test]
    fn soft_clip_keeps_overlaps_below_full_scale() {
        // Mathematically the curve stays strictly below 1.0, but for large inputs
        // tanh rounds to exactly 1.0 in f32. Full scale itself is still safe.
        for sample in [0.9, 1.0, 1.5, 3.0, 100.0] {
            let clipped = soft_clip(sample);
            assert!(clipped <= 1.0, "soft_clip({sample}) = {clipped}");
            assert_eq!(soft_clip(-sample), -clipped);
        }
        assert!(
            soft_clip(1.0) < 1.0,
            "a moderate overlap should keep some headroom"
        );
    }

    #[test]
    fn soft_clip_is_smooth_and_increasing() {
        // No jump at the threshold, and louder input never comes out quieter.
        let samples: Vec<f32> = (0..=400).map(|i| i as f32 / 100.0).collect();
        for pair in samples.windows(2) {
            let (a, b) = (soft_clip(pair[0]), soft_clip(pair[1]));
            assert!(
                b >= a,
                "soft_clip decreased between {} and {}",
                pair[0],
                pair[1]
            );
            assert!(
                b - a <= 0.01 + f32::EPSILON,
                "soft_clip jumped between {} and {}",
                pair[0],
                pair[1]
            );
        }
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

        assert!(matches!(result, Err(PipelineError::Io(_))));
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
    fn idle_speaker_mixes_to_silence() {
        let mut speakers = HashMap::from([(7, Speaker::new().unwrap())]);

        let frame = mix_frame(&mut speakers).unwrap();

        assert_eq!(frame, [0; MIX_FRAME_SAMPLES]);
    }
}
