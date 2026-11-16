# Image resize estimator

## Human summary

Test how resizing images affects the length of their data URLs, since that limit is what Murmur messages care about.

Goal is to estimate scaling factor `k`, such that `output length ∝ scale^k` (where `scale` is the % you adjust both width & height, e.g. 50% scale is half width & half height). That way, you can roughly determine your scaling factor given the desired output length you want (e.g. if `k=2`, then scaling by `0.5x` would reduce length by `4x`).

In practice, k varies. It is ~1.4-1.6 for photos, 1.6-2.1 for screenshots of text, and 2.6-2.8 for noise. Assuming length is proportional to pixel count (`k=2`) will under-compress on images (yielding an image that is over-budget and must be resized again), whereas it will over-compress on noise (yielding an under-budget image). Our code aims to be under budget to avoid repeated re-encoding, so we chose approx 2 with a 95% margin, which works out to a `k` of ~1.86. (Technically, our code's `k` is dependent on image size vs budget, but I'm not particularly interested in changing it).

It seems like a decent value for now.

## Summary

Measures how the length of mumbo's image data URLs scales with image size, and
compares ways of guessing how far to shrink an image so it fits the message
budget. Use it to tune `resize_image_to_fit` in `crates/mumble-messages`, and
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
