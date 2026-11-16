//! Measures how the length of mumbo's image data URLs scales with image size,
//! and compares strategies for guessing how far to shrink an image so it fits
//! a budget, to tune `resize_image_to_fit` in crates/mumble-messages.
//!
//! Times are wall time, so run with --release for meaningful ones.
//!
//! Encodes with mumbo's own `smallest_data_url`, so results track any change
//! to the encoder (formats, JPEG quality).

use image::{DynamicImage, RgbImage, imageops::FilterType};
use mumble_messages::{MAX_IMAGE_HEIGHT, MAX_IMAGE_WIDTH, smallest_data_url};

/// Image budgets to test: the whole image limit of a default Murmur server
/// (131072, less some text), and smaller shares as when several images split it.
const BUDGETS: [usize; 3] = [126_000, 60_000, 25_000];

/// Scales at which to measure how length grows with size.
const SCALES: [f64; 5] = [0.8, 0.6, 0.4, 0.25, 0.1];

/// Encodes before giving up, as in mumble-messages.
const MAX_ENCODES: usize = 9;

#[derive(Clone, Copy)]
enum Strategy {
    /// Assume length ∝ scale^k for a fixed k.
    Fixed { k: f64, padding: f64 },
    /// Encode a thumbnail at `probe` scale first, and measure k from it and
    /// the full-size encode.
    Probe { probe: f64, padding: f64 },
}

const STRATEGIES: [(&str, Strategy); 4] = [
    ("fixed k=2, padding 0.95", Strategy::Fixed { k: 2.0, padding: 0.95 }),
    ("fixed k=1.6, padding 0.95", Strategy::Fixed { k: 1.6, padding: 0.95 }),
    ("probe 0.25, padding 0.97", Strategy::Probe { probe: 0.25, padding: 0.97 }),
    ("probe 0.5, padding 0.97", Strategy::Probe { probe: 0.5, padding: 0.97 }),
];

fn encoded_len(img: &DynamicImage) -> usize {
    smallest_data_url(img).expect("encoding should succeed").len()
}

fn scaled(img: &DynamicImage, scale: f64) -> DynamicImage {
    let width = ((img.width() as f64 * scale).round() as u32).max(1);
    let height = ((img.height() as f64 * scale).round() as u32).max(1);
    img.resize_exact(width, height, FilterType::Lanczos3)
}

/// `img` shrunk to mumbo's pixel cap, as mumble-messages does before encoding.
fn capped(img: &DynamicImage) -> DynamicImage {
    let scale = (MAX_IMAGE_WIDTH as f64 / img.width() as f64)
        .min(MAX_IMAGE_HEIGHT as f64 / img.height() as f64)
        .min(1.0);
    if scale < 1.0 { scaled(img, scale) } else { img.clone() }
}

struct Outcome {
    /// Encodes after the full-size one.
    encodes: usize,
    /// Pixels encoded after the full-size encode, in full-size encodes; a
    /// proxy for time.
    work: f64,
    /// Final length as a fraction of the budget.
    fill: f64,
    /// Wall time, not counting the full-size encode.
    time: std::time::Duration,
}

/// Shrinks `full` (whose encoding is `full_len` long and over `budget`) the
/// way mumble-messages does: guess a scale, and shrink further until it fits.
fn shrink(full: &DynamicImage, full_len: usize, budget: usize, strategy: Strategy) -> Outcome {
    let start = std::time::Instant::now();
    let mut outcome = Outcome { encodes: 0, work: 0.0, fill: 0.0, time: Default::default() };
    let (k, padding) = match strategy {
        Strategy::Fixed { k, padding } => (k, padding),
        Strategy::Probe { probe, padding } => {
            let probe_len = encoded_len(&scaled(full, probe));
            outcome.encodes += 1;
            outcome.work += probe * probe;
            // Tiny images' fixed overhead makes the measurement meaningless.
            let k = ((probe_len as f64 / full_len as f64).ln() / probe.ln()).clamp(1.0, 3.0);
            (k, padding)
        }
    };
    let (mut scale, mut len) = (1.0, full_len);
    while len > budget && outcome.encodes < MAX_ENCODES {
        scale *= (budget as f64 * padding / len as f64).powf(1.0 / k);
        len = encoded_len(&scaled(full, scale));
        outcome.encodes += 1;
        outcome.work += scale * scale;
    }
    outcome.fill = len as f64 / budget as f64;
    outcome.time = start.elapsed();
    outcome
}

/// Built-in samples, for when no images are given, and as extremes.
fn synthetic() -> Vec<(String, DynamicImage)> {
    let mut seed: u32 = 1;
    let mut random = move || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (seed >> 16) as u8
    };
    let (width, height) = (MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT);
    let noise = RgbImage::from_fn(width, height, |_, _| image::Rgb([random(), random(), random()]));
    // Grain compresses badly, much like noise; real photos scale far better
    // (pass some in), so this is a second extreme rather than a stand-in.
    let grainy = RgbImage::from_fn(width, height, |x, y| {
        let grain = (random() % 16) as u32;
        image::Rgb([
            (x * 239 / width + grain) as u8,
            (y * 239 / height + grain) as u8,
            (128 + grain) as u8,
        ])
    });
    vec![
        ("noise (worst case)".into(), DynamicImage::ImageRgb8(noise)),
        ("grainy gradient".into(), DynamicImage::ImageRgb8(grainy)),
    ]
}

fn main() {
    let mut samples = synthetic();
    for path in std::env::args().skip(1) {
        let img = image::open(&path).unwrap_or_else(|e| panic!("can't open {path}: {e}"));
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        samples.push((name, img));
    }

    println!("Length vs. scale, as the exponent k in length ∝ scale^k (2 = proportional to pixels):\n");
    print!("{:<28} {:>10} {:>9} {:>9}", "image (capped size)", "full len", "cap time", "enc time");
    for scale in SCALES {
        print!(" {:>6}", format!("k@{scale}"));
    }
    println!();
    let mut encoded = vec![];
    for (name, img) in &samples {
        let start = std::time::Instant::now();
        let full = capped(img);
        let cap_time = start.elapsed();
        let start = std::time::Instant::now();
        let full_len = encoded_len(&full);
        let encode_time = start.elapsed();
        print!(
            "{:<28} {full_len:>10} {:>9.1?} {:>9.1?}",
            format!("{name} ({}x{})", full.width(), full.height()),
            cap_time,
            encode_time
        );
        for scale in SCALES {
            let ratio = encoded_len(&scaled(&full, scale)) as f64 / full_len as f64;
            print!(" {:>6.2}", ratio.ln() / scale.ln());
        }
        println!();
        encoded.push((name, full, full_len));
    }

    println!("\nShrinking to fit, over every image and budget {BUDGETS:?} it doesn't already fit:\n");
    println!(
        "{:<28} {:>13} {:>5} {:>6} {:>10} {:>9} {:>10}",
        "strategy", "mean encodes", "max", "work", "mean fill", "min fill", "mean time"
    );
    for (label, strategy) in STRATEGIES {
        let outcomes: Vec<Outcome> = encoded
            .iter()
            .flat_map(|(_, full, full_len)| {
                BUDGETS
                    .iter()
                    .filter(|&&budget| *full_len > budget)
                    .map(|&budget| shrink(full, *full_len, budget, strategy))
                    .collect::<Vec<_>>()
            })
            .collect();
        let n = outcomes.len() as f64;
        println!(
            "{label:<28} {:>13.2} {:>5} {:>6.2} {:>9.0}% {:>8.0}% {:>10.1?}",
            outcomes.iter().map(|o| o.encodes as f64).sum::<f64>() / n,
            outcomes.iter().map(|o| o.encodes).max().unwrap_or(0),
            outcomes.iter().map(|o| o.work).sum::<f64>() / n,
            outcomes.iter().map(|o| o.fill).sum::<f64>() / n * 100.0,
            outcomes.iter().map(|o| o.fill).fold(f64::INFINITY, f64::min) * 100.0,
            outcomes.iter().map(|o| o.time).sum::<std::time::Duration>() / outcomes.len().max(1) as u32,
        );
    }
}
