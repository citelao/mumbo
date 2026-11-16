use base64::{Engine, engine::general_purpose::STANDARD};
use data_url::DataUrl;
use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, imageops::FilterType};
use mumble_protocol::state::MumbleState;
use std::error::Error;
use std::io::Cursor;

// Maximum allowed image dimensions.
// Bigger than display size, assuming 2x scaling for high-DPI displays.
pub const MAX_IMAGE_WIDTH: u32 = 1600;
pub const MAX_IMAGE_HEIGHT: u32 = 1200;

/// The largest size an image is displayed at, in CSS/Qt logical pixels;
/// smaller images keep theirs. Keep in sync with MainPage.module.scss.
const MAX_DISPLAY_WIDTH: u32 = 800;
const MAX_DISPLAY_HEIGHT: u32 = 600;

/// JPEG quality for resized images.
const JPEG_QUALITY: u8 = 85;

// The official Mumble client's defaults, for anything the server's
// ServerConfig leaves out (Murmur always sends all three):
// https://github.com/mumble-voip/mumble/blob/ccf405905f8d14ef84f8bc9d8959c6f4b9881b8e/src/mumble/Global.cpp#L119-L121
// https://github.com/mumble-voip/mumble/blob/ccf405905f8d14ef84f8bc9d8959c6f4b9881b8e/src/mumble/MainWindow.cpp#L3588-L3590
// https://github.com/mumble-voip/mumble/blob/ccf405905f8d14ef84f8bc9d8959c6f4b9881b8e/src/mumble/Messages.cpp#L210-L215
const DEFAULT_ALLOW_HTML: bool = true;
const DEFAULT_MESSAGE_LENGTH: usize = 5000;
const DEFAULT_IMAGE_MESSAGE_LENGTH: usize = 131072;

/// The server's text message limits, copied out of `MumbleState` so messages
/// can be sanitized off the main thread without holding its lock.
#[derive(Clone, Copy, Debug)]
pub struct MessageLimits {
    pub allow_html: bool,
    /// Max length of a message's text, not counting image data. 0 is
    /// unlimited, as on the server.
    pub message_length: usize,
    /// Max length of a message including image data. 0 is unlimited.
    pub image_message_length: usize,
}

impl MessageLimits {
    pub fn from_state(state: &MumbleState) -> Self {
        Self {
            allow_html: state.allow_html.unwrap_or(DEFAULT_ALLOW_HTML),
            message_length: state.message_length.map_or(DEFAULT_MESSAGE_LENGTH, |l| l as usize),
            image_message_length: state
                .image_message_length
                .map_or(DEFAULT_IMAGE_MESSAGE_LENGTH, |l| l as usize),
        }
    }
}

/// Fit dimensions within the given maximum width and height, preserving aspect ratio.
fn fit((width, height): (u32, u32), (max_width, max_height): (u32, u32)) -> (u32, u32) {
    let scale = (max_width as f64 / width as f64)
        .min(max_height as f64 / height as f64)
        .min(1.0);
    let width = (width as f64 * scale).round() as u32;
    let height = (height as f64 * scale).round() as u32;
    (width.max(1), height.max(1))
}

#[derive(Debug, Clone)]
struct ResizeResult {
    pub url: String,
    pub height: u32,
    pub width: u32,
}

/// Encode `image` as a data URL at most `budget` long, shrinking it from `full`
/// (its full-size encoding) as needed. Best effort: if it can't, returns the
/// closest it got.
fn resize_image_to_fit(
    image: &DynamicImage,
    full: ResizeResult,
    budget: usize,
) -> Result<ResizeResult, image::ImageError> {
    let budget_f = budget as f64;
    let ResizeResult { url: mut best_url, width: mut best_width, height: mut best_height } = full;
    let mut best_scale_factor = best_width as f64 / image.width() as f64;

    const MAX_ITER: usize = 8;
    for _ in 0..MAX_ITER {
        if best_url.len() <= budget {
            break;
        }

        // How badly the current encoding exceeds the budget.
        let overshoot = best_url.len() as f64 / budget_f;

        // Scale the image down by overshoot, with some padding. Rescaling is
        // non-linear, but this should be a decent approximation. Since rescale
        // is applied to width and height, the scale factor should be the sqrt
        // of this overshoot.
        const RESCALE_PADDING: f64 = 0.95;
        let additional_scale_factor = ((1.0 / overshoot) * RESCALE_PADDING).sqrt();
        best_scale_factor *= additional_scale_factor;

        // Always resize the *source* image for best data quality.
        let (width_f, height_f) = (image.width() as f64 * best_scale_factor, image.height() as f64 * best_scale_factor);
        let (width, height) = (width_f.max(1.0) as u32, height_f.max(1.0) as u32);
        let resized_image = image.resize_exact(width, height, FilterType::Lanczos3);
        let resized_url = smallest_data_url(&resized_image)?;

        best_url = resized_url;
        best_height = height;
        best_width = width;
    }

    Ok(ResizeResult { url: best_url, width: best_width, height: best_height })
}

/// Encode `img` as a PNG or JPEG data URL, whichever is shorter; JPEG only if
/// it's opaque, since JPEG has no alpha channel. Public so
/// contrib/image-resize-estimator measures exactly what we send.
pub fn smallest_data_url(img: &DynamicImage) -> Result<String, image::ImageError> {
    let mut png = Vec::new();
    img.write_with_encoder(PngEncoder::new(&mut png))?;
    let png_url = || format!("data:image/png;base64,{}", STANDARD.encode(&png));

    // As JPEG, transparent pixels would come out as whatever color they
    // store, often black (e.g. the shadow in a macOS window screenshot).
    if img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] < u8::MAX) {
        return Ok(png_url());
    }

    let mut jpeg = Vec::new();
    DynamicImage::ImageRgb8(img.to_rgb8())
        .write_with_encoder(JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY))?;

    Ok(if png.len() <= jpeg.len() {
        png_url()
    } else {
        format!("data:image/jpeg;base64,{}", STANDARD.encode(&jpeg))
    })
}

/// The message that sends `message` (HTML from the chat editor), as long as the
/// server would accept it.
pub fn prepare_text_message(limits: &MessageLimits, message: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    // Images are sent on their own, with `prepare_image_message`. A literal
    // "<img" only appears in tags; in text it's escaped as "&lt;img".
    if message.contains("<img") {
        return Err("Images must be sent on their own".into());
    }

    // Rejects what the server would; see `Server::isTextAllowed`:
    // https://github.com/mumble-voip/mumble/blob/ccf405905f8d14ef84f8bc9d8959c6f4b9881b8e/src/murmur/Server.cpp#L2497-L2555
    // Murmur counts UTF-16 code units; bytes are never fewer.
    //
    // TODO: check messages' length when HTML isn't allowed, which Murmur
    // measures after stripping it to plain text.
    let too_long = |limit: usize| limit != 0 && message.len() > limit;
    if too_long(limits.message_length) || too_long(limits.image_message_length) {
        return Err("This message is too long to send".into());
    }
    Ok(message.to_string())
}

/// The message that sends `data_url`'s image on its own, shrunk to fit the
/// server's limit.
///
/// Murmur also checks a message's text (everything but the `src`s) against
/// `message_length`, plus ~49 characters of its own markup (measured with
/// contrib/text-message-tester). The tag's ~40 characters are far under any
/// real limit, so that isn't checked here.
pub fn prepare_image_message(limits: &MessageLimits, data_url: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    // Murmur would strip the `<img>`, leaving nothing.
    if !limits.allow_html {
        return Err("This server doesn't allow images".into());
    }

    let (body, _fragment) = DataUrl::process(data_url)?.decode_to_vec()?;
    // Detects the format from the data itself.
    let reader = ImageReader::new(Cursor::new(&body)).with_guessed_format()?;
    let format = reader.format().ok_or("Unrecognized image format")?;
    // Rebuilt rather than sent as given: base64 can't contain a `"` that would
    // end the `src` attribute early.
    let original_url = format!("data:{};base64,{}", format.to_mime_type(), STANDARD.encode(&body));

    // `0` means unlimited size: send it as is.
    let is_unlimited_image_size = limits.image_message_length == 0;
    if is_unlimited_image_size {
        return Ok(format!(r#"<img src="{original_url}" />"#));
    }

    // Turn the image upright if its EXIF data says it's stored rotated or
    // flipped (common for phone photos).
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);

    // Encoding large images takes real clock time (e.g. ~2s for a 3200x3200
    // PNG), so start with a sensible max size.
    let (width, height) = fit((image.width(), image.height()), (MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT));
    let is_full_size = (width, height) == (image.width(), image.height());
    let url = if is_full_size {
        if format == ImageFormat::Gif {
            // For full-size GIFs, keep as is to preserve animation.
            original_url
        } else {
            smallest_data_url(&image)?
        }
    } else {
        smallest_data_url(&image.resize_exact(width, height, FilterType::Lanczos3))?
    };

    let img_tag = |url: &str, width: u32, height: u32| {
        format!(r#"<img src="{url}" width="{width}" height="{height}" />"#)
    };
    // The rest of the tag counts too. Displayed sizes are at most MAX_DISPLAY_*,
    // so never have more digits.
    let tag_length = img_tag("", MAX_DISPLAY_WIDTH, MAX_DISPLAY_HEIGHT).len();
    let budget = limits.image_message_length.saturating_sub(tag_length);
    let resized = resize_image_to_fit(&image, ResizeResult { url, width, height }, budget)?;
    if resized.url.len() > budget {
        return Err("This image is too large to send".into());
    }

    let (width, height) = fit((resized.width, resized.height), (MAX_DISPLAY_WIDTH, MAX_DISPLAY_HEIGHT));
    let message = img_tag(&resized.url, width, height);
    log::info!("Sending image: {} characters", message.len());
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    const LIMITS: MessageLimits = MessageLimits {
        allow_html: true,
        message_length: DEFAULT_MESSAGE_LENGTH,
        image_message_length: DEFAULT_IMAGE_MESSAGE_LENGTH,
    };

    /// A smooth gradient: compresses well, like most screenshots.
    fn gradient(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x * 255 / width) as u8, (y * 255 / height) as u8, 128])
        }))
    }

    /// Pseudo-random pixels: compresses terribly, the worst case for the budget.
    fn noise(width: u32, height: u32) -> DynamicImage {
        let mut seed: u32 = 1;
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |_, _| {
            let mut next = || {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (seed >> 16) as u8
            };
            image::Rgb([next(), next(), next()])
        }))
    }

    fn png_data_url(img: &DynamicImage) -> String {
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        format!("data:image/png;base64,{}", STANDARD.encode(&png))
    }

    /// The `src` of each `<img>` in `html`, decoded.
    fn sent_images(html: &str) -> Vec<DynamicImage> {
        html.split(r#"src=""#)
            .skip(1)
            .map(|rest| {
                let src = &rest[..rest.find('"').unwrap()];
                let (body, _) = DataUrl::process(src).unwrap().decode_to_vec().unwrap();
                image::load_from_memory(&body).unwrap()
            })
            .collect()
    }

    /// The `width` and `height` of each `<img>` in `html`.
    fn displayed_sizes(html: &str) -> Vec<(String, String)> {
        let attribute = |tag: &str, name: &str| {
            let rest = &tag[tag.find(&format!(r#" {name}=""#)).unwrap() + name.len() + 3..];
            rest[..rest.find('"').unwrap()].to_string()
        };
        html.split("<img")
            .skip(1)
            .map(|rest| {
                let tag = &rest[..rest.find("/>").unwrap()];
                (attribute(tag, "width"), attribute(tag, "height"))
            })
            .collect()
    }

    #[test]
    fn sends_an_img_tag() {
        let sent = prepare_image_message(&LIMITS, &png_data_url(&gradient(40, 30))).unwrap();
        assert!(sent.starts_with(r#"<img src="data:image/"#), "{sent:.40}");
        assert!(sent.ends_with(r#" width="40" height="30" />"#), "{}", &sent[sent.len() - 30..]);
    }

    #[test]
    fn does_not_upscale_small_images() {
        let sent = prepare_image_message(&LIMITS, &png_data_url(&gradient(100, 50))).unwrap();
        let images = sent_images(&sent);
        assert_eq!((images[0].width(), images[0].height()), (100, 50));
    }

    #[test]
    fn sends_compressible_images_at_2x_for_hidpi_displays() {
        let sent = prepare_image_message(&LIMITS, &png_data_url(&gradient(1600, 1200))).unwrap();
        let images = sent_images(&sent);
        assert_eq!((images[0].width(), images[0].height()), (1600, 1200));
        assert_eq!(displayed_sizes(&sent), [("800".into(), "600".into())]);
    }

    /// Shrunk images display at their new size rather than being stretched
    /// back up to the original's.
    #[test]
    fn does_not_stretch_shrunk_images() {
        let sent = prepare_image_message(&LIMITS, &png_data_url(&noise(1600, 1200))).unwrap();
        let image = &sent_images(&sent)[0];
        assert!(image.width() < 800);
        assert_eq!(displayed_sizes(&sent), [(image.width().to_string(), image.height().to_string())]);
    }

    /// Noise is the worst case: shrinking smooths it, so it lands well under
    /// the budget (real photos land at ~95%; see contrib/image-resize-estimator).
    #[test]
    fn shrinks_incompressible_images_to_fit() {
        let sent = prepare_image_message(&LIMITS, &png_data_url(&noise(1600, 1200))).unwrap();
        assert!(sent.len() <= LIMITS.image_message_length, "{}", sent.len());
    }

    #[test]
    fn rejects_images_too_large_to_shrink_to_fit() {
        let limits = MessageLimits { image_message_length: 100, ..LIMITS };
        assert!(prepare_image_message(&limits, &png_data_url(&noise(64, 64))).is_err());
    }

    #[test]
    fn sends_images_unchanged_when_unlimited() {
        let limits = MessageLimits { image_message_length: 0, ..LIMITS };
        let url = png_data_url(&noise(1600, 1200));
        assert_eq!(prepare_image_message(&limits, &url).unwrap(), format!(r#"<img src="{url}" />"#));
    }

    /// Murmur strips the `<img>`, so nothing would arrive.
    #[test]
    fn rejects_images_when_the_server_disallows_html() {
        let limits = MessageLimits { allow_html: false, ..LIMITS };
        let error = prepare_image_message(&limits, &png_data_url(&gradient(40, 30))).unwrap_err();
        assert!(error.to_string().contains("images"), "{error}");
    }

    /// The `src` is quoted as given, so it must not be able to close the quote.
    #[test]
    fn rejects_data_urls_that_are_not_images() {
        assert!(prepare_image_message(&LIMITS, r#"data:image/png,"><script>"#).is_err());
    }

    #[test]
    fn sends_text_up_to_message_length() {
        let message = "a".repeat(LIMITS.message_length);
        assert_eq!(prepare_text_message(&LIMITS, &message).unwrap(), message);
    }

    #[test]
    fn rejects_text_too_long_to_send() {
        assert!(prepare_text_message(&LIMITS, &"a".repeat(LIMITS.message_length + 1)).is_err());
    }

    /// Images are sent on their own, with `prepare_image_message`.
    #[test]
    fn rejects_images_in_text() {
        let message = format!(r#"hi<img src="{}" />"#, png_data_url(&gradient(40, 30)));
        assert!(prepare_text_message(&LIMITS, &message).is_err());
    }

    /// JPEG has no alpha, and transparent pixels would come out as whatever
    /// color they happen to store (often black): e.g. macOS window shadows.
    #[test]
    fn keeps_transparency() {
        // Noisy, so JPEG would be the smaller encoding.
        let Some(noisy) = noise(400, 300).as_rgb8().cloned() else { unreachable!() };
        let rgba = image::RgbaImage::from_fn(400, 300, |x, y| {
            let [r, g, b] = noisy.get_pixel(x, y).0;
            image::Rgba([r, g, b, if x < 200 { 0 } else { 255 }])
        });
        let sent = sent_images(&prepare_image_message(&LIMITS, &png_data_url(&DynamicImage::ImageRgba8(rgba))).unwrap());
        assert_eq!(sent[0].to_rgba8().get_pixel(10, 10).0[3], 0);
    }

    /// Phone photos are often stored sideways with an EXIF tag saying so.
    #[test]
    fn applies_exif_orientation() {
        use image::ImageEncoder;
        // A big-endian TIFF header and one IFD entry: Orientation (0x0112),
        // SHORT, 1 value, 6 (rotate 90° clockwise to display).
        let exif = [
            b"MM\0\x2a\0\0\0\x08".as_slice(),
            &[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0],
        ]
        .concat();
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new(&mut jpeg);
        encoder.set_exif_metadata(exif).unwrap();
        let stored = gradient(100, 50);
        encoder.write_image(stored.as_bytes(), 100, 50, image::ExtendedColorType::Rgb8).unwrap();

        let url = format!("data:image/jpeg;base64,{}", STANDARD.encode(&jpeg));
        let sent = sent_images(&prepare_image_message(&LIMITS, &url).unwrap());
        assert_eq!((sent[0].width(), sent[0].height()), (50, 100));
    }

    /// Re-encoding would keep only the first frame.
    #[test]
    fn sends_gifs_that_fit_as_is() {
        use image::codecs::gif::GifEncoder;
        let mut gif = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut gif);
            for shade in [0u8, 255] {
                let frame = image::RgbaImage::from_pixel(32, 32, image::Rgba([shade, shade, shade, 255]));
                encoder.encode_frame(image::Frame::new(frame)).unwrap();
            }
        }
        let url = format!("data:image/gif;base64,{}", STANDARD.encode(&gif));
        let sent = prepare_image_message(&LIMITS, &url).unwrap();
        assert!(sent.contains(&url), "{sent:.60}");
    }
}
