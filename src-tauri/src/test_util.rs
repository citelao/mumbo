//! Signal helpers shared by tests in more than one module.

const SAMPLE_RATE: u32 = 48_000;

/// `samples` samples of a half-amplitude sine at `frequency`.
pub fn sine(frequency: f32, sample_rate: u32, samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|i| (i as f32 / sample_rate as f32 * frequency * std::f32::consts::TAU).sin() * 0.5)
        .collect()
}

/// Amplitude of the `frequency` component of 48 kHz `samples` (a single DFT bin).
pub fn tone_strength(samples: &[f32], frequency: f32) -> f32 {
    let (re, im) = samples
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (n, &x)| {
            let phase = std::f32::consts::TAU * frequency * n as f32 / SAMPLE_RATE as f32;
            (re + x * phase.cos(), im - x * phase.sin())
        });
    2.0 * (re * re + im * im).sqrt() / samples.len() as f32
}
