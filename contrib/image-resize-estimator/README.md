# Image resize estimator

Measures how the length of mumbo's image data URLs scales with image size, and
compares ways of guessing how far to shrink an image so it fits the message
budget. Use it to tune `resize_image_to_fit` in `src-tauri/src/messages.rs`, and
re-run it when the encoder (formats, JPEG quality) or the pixel cap changes: it
encodes with mumbo's own `smallest_data_url`.

## Usage

```sh
cargo run --release -p image-resize-estimator -- path/to/screenshot.png path/to/photo.jpg
```

Pass real screenshots and photos: the built-in samples are only extremes
(random noise, and a heavily grainy gradient), which compress far worse than
real images.

It prints:

- **k at each scale**: in `length ∝ scale^k`, how fast each image's encoded
  length shrinks with its size (2 would be proportional to pixel count).
- **For each strategy**, over every image and budget it doesn't already fit:
  encodes after the full-size one, pixel work (in full-size encodes, a proxy for
  time), and how full of the budget the result is.

## Findings (2026-11)

- k varies a lot by image: about 1.4–1.6 for photos, 1.6–2.1 for dense text,
  2.6–2.8 for noise. No fixed k guesses well; assuming 2 overshoots on photos
  (several retries), and 1.6 badly undershoots text.
- Measuring k per image from a 0.25-scale thumbnail (1/16 the pixels) plus the
  full-size encode needed anyway fills the budget more reliably, especially for
  noise-like images, in about half the pixel work of assuming k = 2.
- But it doesn't save wall time worth having: shrinking takes ~30 ms either way
  (release build), against ~200 ms to decode a large pasted PNG, cap it to
  1600×1200, and encode it once. On real photos and screenshots, k = 2 already
  fills ~95%+.
