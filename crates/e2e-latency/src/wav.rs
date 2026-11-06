use std::error::Error;

pub struct Wav {
    pub sample_rate: u32,
    /// Mono samples in [-1, 1].
    pub samples: Vec<f32>,
}

/// Parses a mono, 16-bit PCM WAV file (what `espeak-ng -w` writes).
pub fn parse(bytes: &[u8]) -> Result<Wav, Box<dyn Error>> {
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut format = None;
    let mut pos = 12;
    while let (Some(id), Some(len)) = (bytes.get(pos..pos + 4), bytes.get(pos + 4..pos + 8)) {
        let len = u32::from_le_bytes(len.try_into().unwrap()) as usize;
        let body_start = pos + 8;
        // Streaming writers (espeak-ng to a pipe) leave the data length as
        // 0 or 0xFFFFFFFF: the data runs to the end of the file.
        let body_end = (body_start + len).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                let tag = u16::from_le_bytes(body[0..2].try_into()?);
                let channels = u16::from_le_bytes(body[2..4].try_into()?);
                let rate = u32::from_le_bytes(body[4..8].try_into()?);
                let bits = u16::from_le_bytes(body[14..16].try_into()?);
                if (tag, channels, bits) != (1, 1, 16) {
                    return Err(format!("need mono 16-bit PCM, got tag {tag}, {channels} ch, {bits} bits").into());
                }
                format = Some(rate);
            }
            b"data" => {
                let sample_rate = format.ok_or("data chunk before fmt chunk")?;
                let body = if len == 0 || len == u32::MAX as usize { &bytes[body_start..] } else { body };
                let samples = body
                    .chunks_exact(2)
                    .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
                    .collect();
                return Ok(Wav { sample_rate, samples });
            }
            _ => {}
        }
        pos = body_start + len + (len & 1);
    }
    Err("no data chunk".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, samples: &[i16], data_len: u32) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes()); // PCM
        out.extend(1u16.to_le_bytes()); // mono
        out.extend(rate.to_le_bytes());
        out.extend((rate * 2).to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend(data_len.to_le_bytes());
        out.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        out
    }

    #[test]
    fn reads_samples_and_rate() {
        let parsed = parse(&wav(22_050, &[0, 16384, -32768], 6)).unwrap();
        assert_eq!(parsed.sample_rate, 22_050);
        assert_eq!(parsed.samples, [0.0, 0.5, -1.0]);
    }

    #[test]
    fn streamed_files_with_an_unknown_length_run_to_the_end() {
        let parsed = parse(&wav(22_050, &[1, 2, 3], u32::MAX)).unwrap();
        assert_eq!(parsed.samples.len(), 3);
    }

    #[test]
    fn rejects_stereo() {
        let mut bytes = wav(22_050, &[0, 0], 4);
        bytes[22] = 2; // channels
        assert!(parse(&bytes).is_err());
    }
}
