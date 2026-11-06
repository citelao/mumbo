//! Real speech through the app's own mic and playback pipelines, minus the
//! sound card. The sender feeds a speech recording to `MicPipeline` as if it
//! were a microphone, in real time; the receiver runs the `Mixer` (NetEq +
//! Opus) and the playback queue exactly as the app does, with the sound card
//! replaced by a clock that drains the queue at 48 kHz.
//!
//! Per utterance, the timeline is:
//!
//! ```text
//! speech starts ──transmit──▶ first packet sent ──network──▶ first packet
//!   (sender mic)               (sender)                       received ──playback──▶ audible
//! ```
//!
//! The sender announces the first two timestamps over channel chat; the
//! receiver observes the rest. Clocks are shared (same host / VM).

use std::{
    error::Error,
    sync::{Arc, Mutex, atomic::{AtomicU64, Ordering::Relaxed}},
    time::{Duration, Instant},
};

use mumbo_audio::{
    mic::{MicPipeline, apm_config, audio_processing},
    speaker::{Mixer, SPEAKER_SAMPLE_RATE, top_up_output},
};
use mumble_protocol::{client::Connection, state::{MumbleEvent, TextMessage}};
use ringbuf::{HeapRb, traits::{Consumer, Split}};

use crate::{
    common::{connect_with_retry, env_or, now_nanos},
    wav,
};

/// Silence before each utterance, so the mic processing settles first.
const LEAD_SILENCE: Duration = Duration::from_secs(1);
/// Silence after: longer than the voice gate's hold, so the burst ends, the
/// listener's jitter buffer drains, and the next utterance starts cold.
const TAIL_SILENCE: Duration = Duration::from_secs(2);
/// No packets for this long means the next one starts a new utterance. Longer
/// than any pause inside the sentence, shorter than the silence between them.
const UTTERANCE_GAP: Duration = Duration::from_secs(2);
/// Samples quieter than this (as a fraction of full scale) aren't "heard".
const AUDIBLE: f32 = 0.02;

/// What the app does each 5 ms: top the playback queue up to this much audio.
const PLAYBACK_QUEUE_SAMPLES: usize = SPEAKER_SAMPLE_RATE as usize / 1000 * 40;

fn nanos_after(base: u64, samples: usize, rate: u32) -> u64 {
    base + (samples as f64 / rate as f64 * 1e9) as u64
}

pub async fn sender() -> Result<(), Box<dyn Error>> {
    let wav = wav::parse(&std::fs::read(env_or("SPEECH_WAV", "/speech.wav".to_string()))?)?;
    let utterances: usize = env_or("SPEECH_UTTERANCES", 5);
    let noise_suppression: bool = env_or("NOISE_SUPPRESSION", true);
    let rate = wav.sample_rate;
    let onset = wav.samples.iter().position(|s| s.abs() > AUDIBLE).ok_or("the speech file is silent")?;

    let lead = (LEAD_SILENCE.as_secs_f64() * rate as f64) as usize;
    let tail = (TAIL_SILENCE.as_secs_f64() * rate as f64) as usize;
    let per_utterance = lead + wav.samples.len() + tail;
    let signal: Vec<f32> = (0..utterances)
        .flat_map(|_| {
            std::iter::repeat_n(0.0, lead)
                .chain(wav.samples.iter().copied())
                .chain(std::iter::repeat_n(0.0, tail))
        })
        .collect();

    // Held, unread, for the whole run: sending chat unwraps a send to `events`,
    // which panics if the receiver was dropped.
    let Connection { client, events: _events, .. } = connect_with_retry("speech-sender".to_string()).await;

    let first_packet = Arc::new(AtomicU64::new(0));
    let mut sink = {
        let (client, first_packet) = (client.clone(), first_packet.clone());
        move |opus: &[u8], frame: u64, is_terminator: bool| {
            // Only the first call after a reset sticks.
            let _ = first_packet.compare_exchange(0, now_nanos(), Relaxed, Relaxed);
            if let Err(e) = client.send_audio(opus, frame, is_terminator) {
                eprintln!("failed to send audio: {e}");
            }
        }
    };

    let mut apm = audio_processing(apm_config());
    apm.set_stream_delay_ms(50)?;
    let mut pipeline = MicPipeline::new(rate)?;

    eprintln!("sender: {utterances} utterances, {} ms of speech each, mic at {rate} Hz", wav.samples.len() * 1000 / rate as usize);
    let (started, wall_start) = (Instant::now(), now_nanos());
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    let (mut fed, mut announced_start, mut announced_first) = (0usize, 0usize, 0usize);
    while fed < signal.len() {
        ticker.tick().await;
        let due = ((started.elapsed().as_secs_f64() * rate as f64) as usize).min(signal.len());

        // A new utterance starts at the beginning of its lead-in silence.
        if announced_start < utterances && due >= announced_start * per_utterance {
            first_packet.store(0, Relaxed);
            let speech_start = nanos_after(wall_start, announced_start * per_utterance + lead + onset, rate);
            client.send_channel_message(format!("e2e:start {announced_start} {speech_start}"), None)?;
            announced_start += 1;
        }

        pipeline.process(&signal[fed..due], &mut apm, noise_suppression, &mut sink)?;
        fed = due;

        let sent = first_packet.load(Relaxed);
        if sent != 0 && announced_first < announced_start {
            client.send_channel_message(format!("e2e:first {announced_first} {sent}"), None)?;
            announced_first += 1;
        }
    }
    eprintln!("sender: done");

    // Stay connected until the receiver finishes and compose tears us down.
    tokio::time::sleep(Duration::from_secs(3600)).await;
    Ok(())
}

/// What the sender announced about one utterance.
#[derive(Default, Clone, Copy)]
struct Announced {
    speech_start: Option<u64>,
    first_sent: Option<u64>,
}

fn parse_announcement(content: &str, announced: &mut Vec<Announced>) {
    let mut parts = content.split_whitespace();
    let (Some(kind), Some(Ok(utterance)), Some(Ok(nanos))) =
        (parts.next(), parts.next().map(str::parse::<usize>), parts.next().map(str::parse::<u64>))
    else {
        return;
    };
    if announced.len() <= utterance {
        announced.resize(utterance + 1, Announced::default());
    }
    match kind {
        "e2e:start" => announced[utterance].speech_start = Some(nanos),
        "e2e:first" => announced[utterance].first_sent = Some(nanos),
        _ => {}
    }
}

pub async fn receiver() -> Result<(), Box<dyn Error>> {
    let utterances: usize = env_or("SPEECH_UTTERANCES", 5);
    let max_total_ms: f64 = env_or("MAX_TOTAL_MS", 1000.0);
    let timeout = Duration::from_secs(env_or("PROBE_TIMEOUT_SECS", 90));

    let Connection { client, mut audio, mut events, .. } =
        connect_with_retry("speech-receiver".to_string()).await;
    eprintln!("receiver connected; waiting for {utterances} utterances");

    // Timestamp each utterance's first packet as it arrives from the server,
    // then pass everything on to the mixer untouched.
    let arrivals = Arc::new(Mutex::new(Vec::<u64>::new()));
    let (packets_tx, packets_rx) = tokio::sync::mpsc::channel(512);
    tokio::spawn({
        let arrivals = arrivals.clone();
        async move {
            let mut last_packet: Option<u64> = None;
            while let Some(packet) = audio.recv().await {
                let now = now_nanos();
                // Pauses inside a sentence end a burst (terminator) and start
                // another, so the gap decides what is a new utterance.
                let gap = last_packet.map_or(u64::MAX, |last| now - last);
                if gap > UTTERANCE_GAP.as_nanos() as u64 {
                    arrivals.lock().unwrap().push(now);
                }
                last_packet = Some(now);
                if packets_tx.send(packet).await.is_err() {
                    break;
                }
            }
        }
    });

    // The sender's timestamps arrive as chat messages.
    let announced = Arc::new(Mutex::new(Vec::<Announced>::new()));
    tokio::spawn({
        let (announced, state) = (announced.clone(), client.state());
        async move {
            let mut seen = 0;
            while let Some(event) = events.recv().await {
                if !matches!(event, MumbleEvent::ChatMessage) {
                    continue;
                }
                let state = state.lock().unwrap();
                for message in &state.chat_messages[seen..] {
                    if let TextMessage::ChatTextMessage(chat) = message {
                        parse_announcement(&chat.content, &mut announced.lock().unwrap());
                    }
                }
                seen = state.chat_messages.len();
            }
        }
    });

    // The app's playback loop: every 5 ms, top the queue up from the mixer.
    // The "sound card" drains it at exactly 48 kHz.
    let mut mixer = Mixer::new(packets_rx);
    let mut apm = audio_processing(apm_config());
    apm.set_stream_delay_ms(50)?;
    let (mut queue_in, mut card) = HeapRb::<i16>::new(SPEAKER_SAMPLE_RATE as usize / 5).split();
    let mut ticker = tokio::time::interval(Duration::from_millis(5));
    let (started, wall_start) = (Instant::now(), now_nanos());
    let mut played = 0usize;
    let mut onsets = Vec::<u64>::new();
    while onsets.len() < utterances {
        ticker.tick().await;
        if started.elapsed() > timeout {
            return Err(format!("timed out after hearing {} of {utterances} utterances", onsets.len()).into());
        }
        if !top_up_output(&mut queue_in, PLAYBACK_QUEUE_SAMPLES, &mut mixer, &mut apm)? {
            return Err("connection closed before every utterance was heard".into());
        }

        let due = (started.elapsed().as_secs_f64() * SPEAKER_SAMPLE_RATE as f64) as usize;
        while played < due {
            let sample = card.try_pop().unwrap_or(0) as f32 / i16::MAX as f32;
            if onsets.len() < arrivals.lock().unwrap().len() && sample.abs() > AUDIBLE {
                onsets.push(nanos_after(wall_start, played, SPEAKER_SAMPLE_RATE));
            }
            played += 1;
        }
    }

    // The sender's chat messages can trail the audio slightly.
    for _ in 0..40 {
        let complete = {
            let announced = announced.lock().unwrap();
            announced.len() >= utterances
                && announced.iter().take(utterances).all(|a| a.speech_start.is_some() && a.first_sent.is_some())
        };
        if complete {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let (announced, arrivals) = (announced.lock().unwrap().clone(), arrivals.lock().unwrap().clone());
    let ms = |from: u64, to: u64| (to as f64 - from as f64) / 1e6;
    let mut rows = Vec::new();
    println!("{:>4} {:>10} {:>9} {:>9} {:>8}", "utt", "transmit", "network", "playback", "total");
    for i in 0..utterances {
        let (Some(start), Some(sent)) = (
            announced.get(i).and_then(|a| a.speech_start),
            announced.get(i).and_then(|a| a.first_sent),
        ) else {
            return Err(format!("never heard the sender's timestamps for utterance {i}").into());
        };
        let row = [ms(start, sent), ms(sent, arrivals[i]), ms(arrivals[i], onsets[i]), ms(start, onsets[i])];
        println!("{i:>4} {:>8.1}ms {:>7.1}ms {:>7.1}ms {:>6.1}ms", row[0], row[1], row[2], row[3]);
        rows.push(row);
    }
    let mean = |col: usize| rows.iter().map(|r| r[col]).sum::<f64>() / rows.len() as f64;
    println!("mean {:>8.1}ms {:>7.1}ms {:>7.1}ms {:>6.1}ms", mean(0), mean(1), mean(2), mean(3));
    println!("speech_latency_ms: transmit={:.1} network={:.1} playback={:.1} total={:.1}", mean(0), mean(1), mean(2), mean(3));

    let worst = rows.iter().map(|r| r[3]).fold(0.0, f64::max);
    if worst > max_total_ms {
        return Err(format!("slowest utterance took {worst:.1}ms to be heard; limit {max_total_ms}ms").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcements_fill_in_by_utterance() {
        let mut announced = Vec::new();
        parse_announcement("e2e:first 1 500", &mut announced);
        parse_announcement("e2e:start 1 100", &mut announced);
        parse_announcement("e2e:start 0 7", &mut announced);

        assert_eq!(announced.len(), 2);
        assert_eq!((announced[1].speech_start, announced[1].first_sent), (Some(100), Some(500)));
        assert_eq!((announced[0].speech_start, announced[0].first_sent), (Some(7), None));
    }

    #[test]
    fn unrelated_chat_is_ignored() {
        let mut announced = Vec::new();
        parse_announcement("hello there", &mut announced);
        parse_announcement("e2e:start x y", &mut announced);
        assert!(announced.is_empty());
    }
}
