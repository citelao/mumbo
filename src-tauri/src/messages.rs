use base64::{Engine, engine::general_purpose::STANDARD};
use data_url::DataUrl;
use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, imageops::FilterType};
use lol_html::errors::RewritingError;
use lol_html::html_content::Element;
use lol_html::{element, HandlerResult, HtmlRewriter, Settings};
use mumble_protocol::state::MumbleState;
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
    pub success: bool,

    // Best URL. If successful, this is within budget. If not, the closest we got.
    pub best_url: String,
    pub height: u32,
    pub width: u32,
}

/// A decoded image from the message, and the data URL it came from.
struct SourceImage {
    image: DynamicImage,
    format: ImageFormat,
    url: String,
}

/// Encode `image` as a data URL at most `budget` long, shrinking it from `full`
/// (its full-size encoding) as needed. Best effort.
fn resize_image_to_fit(
    image: &DynamicImage,
    full: ResizeResult,
    budget: usize,
) -> Result<ResizeResult, image::ImageError> {
    let budget_f = budget as f64;
    let ResizeResult { mut best_url, width: mut best_width, height: mut best_height, .. } = full;
    let mut best_scale_factor = best_width as f64 / image.width() as f64;

    const MAX_ITER: usize = 8;
    for _ in 0..MAX_ITER {
        if best_url.len() <= budget {
            return Ok(ResizeResult { success: true, best_url, width: best_width, height: best_height });
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

    let success = best_url.len() <= budget;
    Ok(ResizeResult { success, best_url, width: best_width, height: best_height })
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

/// Runs `on_img` on each `<img>` in `html`, returning the rewritten HTML.
fn rewrite(
    html: &str,
    mut on_img: impl FnMut(&mut Element<'_, '_>) -> HandlerResult,
) -> Result<Vec<u8>, RewritingError> {
    let mut output = vec![];
    let mut rewriter = HtmlRewriter::new(
        Settings::new().append_element_content_handler(element!("img", |el| on_img(el))),
        |c: &[u8]| output.extend_from_slice(c),
    );
    rewriter.write(html.as_bytes())?;
    rewriter.end()?;
    Ok(output)
}

pub fn sanitize_user_message(
    limits: &MessageLimits,
    message: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Images are big; throw an error if trying to send them to a server that
    // doesn't even support them. A literal "<img" only appears in tags; in text
    // it's escaped as "&lt;img".
    //
    // TODO: check messages' length when HTML isn't allowed, which Murmur
    // measures after stripping it to plain text.
    if !limits.allow_html && message.contains("<img") {
        return Err("This server doesn't allow images".into());
    }

    // `0` means unlimited size: send images as they are.
    let is_unlimited_images = limits.image_message_length == 0;

    // Two passes, since how much each image should be re-encoded to depends on
    // all of them: first decode them, then replace them in the same order.
    let mut sources = vec![];
    let mut image_data_length = 0;
    rewrite(message, |el| {
        let src = el.get_attribute("src").unwrap_or_default();
        if !src.starts_with("data:") {
            // The official client won't load remote images.
            return Err("Only embedded (data: URL) images can be sent".into());
        }
        // `get_attribute` decodes entities (e.g. `&amp;`), but our data URLs are
        // base64, so non-issue.
        image_data_length += src.len();
        if !is_unlimited_images {
            let (body, _fragment) = DataUrl::process(&src)?.decode_to_vec()?;
            // Detects the format from the data itself.
            let reader = ImageReader::new(Cursor::new(&body)).with_guessed_format()?;
            let format = reader.format().ok_or("Unrecognized image format")?;
            // Turn the image upright if its EXIF data says it's stored rotated
            // or flipped (common for phone photos).
            let mut decoder = reader.into_decoder()?;
            let orientation = decoder.orientation()?;
            let mut image = DynamicImage::from_decoder(decoder)?;
            image.apply_orientation(orientation);
            sources.push(SourceImage { image, format, url: src });
        }
        Ok(())
    })?;

    // Image space is the remaining space after accounting for text length.
    let text_length = message.len() - image_data_length;
    let space_for_images = limits.image_message_length.saturating_sub(text_length);

    let (output, sent_text_length) = if is_unlimited_images {
        // Sent as is.
        (message.to_string(), text_length)
    } else {
        // Encoding large images takes real clock time (e.g. ~2s for a 3200x3200
        // PNG), so start with a sensible max size.
        let mut results = sources
            .iter()
            .map(|source| {
                let image = &source.image;
                let (width, height) = fit((image.width(), image.height()), (MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT));
                let is_full_size = (width, height) == (image.width(), image.height());
                let best_url = if is_full_size {
                    if source.format == ImageFormat::Gif {
                        // For full-size GIFs, keep as is to preserve animation.
                        source.url.clone()
                    } else {
                        smallest_data_url(image)?
                    }
                } else {
                    smallest_data_url(&image.resize_exact(width, height, FilterType::Lanczos3))?
                };
                Ok(ResizeResult { success: true, best_url, width, height })
            })
            .collect::<Result<Vec<_>, image::ImageError>>()?;

        if results.iter().map(|r| r.best_url.len()).sum::<usize>() > space_for_images {
            // Best-effort, simple resize to fit images within the available
            // space for images. Resize from smallest to largest, since small
            // images shrink less.
            //
            // Shares are by displayed area, so larger images get more bytes.
            let displayed_area = |image: &DynamicImage| {
                let (width, height) = fit((image.width(), image.height()), (MAX_DISPLAY_WIDTH, MAX_DISPLAY_HEIGHT));
                width as u64 * height as u64
            };
            let mut resize_order: Vec<usize> = (0..sources.len()).collect();
            resize_order.sort_by_key(|&i| displayed_area(&sources[i].image));

            let mut remaining_space = space_for_images;
            let mut remaining_area: u64 = sources.iter().map(|s| displayed_area(&s.image)).sum();
            for i in resize_order {
                let image = &sources[i].image;
                let area = displayed_area(image);
                let proportion = area as f64 / remaining_area as f64;
                let budget = (proportion * remaining_space as f64) as usize;
                let resized = resize_image_to_fit(image, results[i].clone(), budget)?;
                if !resized.success {
                    return Err(match sources.len() {
                        1 => "This image is too large to send".into(),
                        _ => "These images are too large to send together; try sending them separately".into(),
                    });
                }
                remaining_space = remaining_space.saturating_sub(resized.best_url.len());
                remaining_area -= area;
                results[i] = resized;
            }
        }

        let sent_image_data_length: usize = results.iter().map(|r| r.best_url.len()).sum();
        let mut results = results.into_iter();
        let output = rewrite(message, |el| {
            let r = results.next().ok_or("Message changed between passes")?;

            // At most MAX_DISPLAY_*.
            let (width, height) = fit((r.width, r.height), (MAX_DISPLAY_WIDTH, MAX_DISPLAY_HEIGHT));

            el.set_attribute("src", &r.best_url)?;
            el.set_attribute("width", &width.to_string())?;
            el.set_attribute("height", &height.to_string())?;
            Ok(())
        })?;
        let output = String::from_utf8(output)?;
        // Includes the width/height we added, which Murmur counts as text.
        let sent_text_length = output.len() - sent_image_data_length;
        (output, sent_text_length)
    };

    // Reject what the server would, as far as we can tell before sending; see
    // `Server::isTextAllowed`:
    // https://github.com/mumble-voip/mumble/blob/ccf405905f8d14ef84f8bc9d8959c6f4b9881b8e/src/murmur/Server.cpp#L2497-L2555
    //
    // Murmur counts UTF-16 code units; bytes are never fewer.
    let too_long = !is_unlimited_images && output.len() > limits.image_message_length;
    // Over message_length, Murmur checks only the text, re-serialized with
    // some markup of its own (e.g. the `<document>` it wraps it in to parse).
    // Measured with contrib/text-message-tester --find-limits. Slightly
    // conservative: that also counted its `<img src="" />` as overhead.
    const XML_CHECK_OVERHEAD: usize = 49;
    let text_too_long = limits.message_length != 0
        && output.len() > limits.message_length
        && sent_text_length + XML_CHECK_OVERHEAD > limits.message_length;
    if too_long || text_too_long {
        return Err("This message is too long to send".into());
    }

    log::info!("Sending message: {} characters, {} of them text", output.len(), sent_text_length);
    Ok(output)
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
    fn keeps_img_self_closing() {
        let message = format!(r#"hi<br /><img src="{}" />"#, png_data_url(&gradient(40, 30)));
        let sent = sanitize_user_message(&LIMITS, &message).unwrap();
        assert!(sent.starts_with("hi<br /><img src=\"data:image/"), "{sent:.40}");
        assert!(sent.ends_with(" />"), "{}", &sent[sent.len() - 10..]);
    }

    #[test]
    fn does_not_upscale_small_images() {
        let message = format!(r#"<img src="{}" />"#, png_data_url(&gradient(100, 50)));
        let sent = sanitize_user_message(&LIMITS, &message).unwrap();
        let images = sent_images(&sent);
        assert_eq!((images[0].width(), images[0].height()), (100, 50));
    }

    #[test]
    fn sends_compressible_images_at_2x_for_hidpi_displays() {
        let message = format!(r#"<img src="{}" />"#, png_data_url(&gradient(1600, 1200)));
        let sent = sanitize_user_message(&LIMITS, &message).unwrap();
        let images = sent_images(&sent);
        assert_eq!((images[0].width(), images[0].height()), (1600, 1200));
        assert_eq!(displayed_sizes(&sent), [("800".into(), "600".into())]);
    }

    /// Shrunk images display at their new size rather than being stretched
    /// back up to the original's.
    #[test]
    fn does_not_stretch_shrunk_images() {
        let message = format!(r#"<img src="{}" />"#, png_data_url(&noise(1600, 1200)));
        let sent = sanitize_user_message(&LIMITS, &message).unwrap();
        let image = &sent_images(&sent)[0];
        assert!(image.width() < 800);
        assert_eq!(displayed_sizes(&sent), [(image.width().to_string(), image.height().to_string())]);
    }

    /// Noise is the worst case: shrinking smooths it, so it lands well under
    /// the budget (real photos land at ~95%; see contrib/image-resize-estimator).
    #[test]
    fn shrinks_incompressible_images_to_fit() {
        let message = format!(r#"<img src="{}" />"#, png_data_url(&noise(1600, 1200)));
        let sent = sanitize_user_message(&LIMITS, &message).unwrap();
        assert!(sent.len() <= LIMITS.image_message_length, "{}", sent.len());
    }

    /// The budget is split by how big images look, not how well they happen to
    /// compress: otherwise a smooth image beside a noisy one gets almost none.
    #[test]
    fn images_of_the_same_size_get_the_same_share() {
        let message = format!(
            r#"<img src="{}" /><img src="{}" />"#,
            png_data_url(&gradient(800, 600)),
            png_data_url(&noise(800, 600))
        );
        let sent = sent_images(&sanitize_user_message(&LIMITS, &message).unwrap());
        assert_eq!((sent[0].width(), sent[0].height()), (800, 600));
    }

    #[test]
    fn sends_several_images_that_fit_as_is() {
        let src = png_data_url(&gradient(400, 300));
        let message = format!(r#"<img src="{src}" /><img src="{src}" />"#);
        let sent = sent_images(&sanitize_user_message(&LIMITS, &message).unwrap());
        let sizes: Vec<_> = sent.iter().map(|i| (i.width(), i.height())).collect();
        assert_eq!(sizes, [(400, 300), (400, 300)]);
    }

    #[test]
    fn shrinks_several_images_to_fit_together() {
        let src = png_data_url(&noise(800, 600));
        let message = format!(r#"<img src="{src}" /><img src="{src}" />"#);
        let sent = sanitize_user_message(&LIMITS, &message).unwrap();
        assert_eq!(sent_images(&sent).len(), 2);
        assert!(sent.len() <= LIMITS.image_message_length, "{}", sent.len());
    }

    /// Splitting the budget only when needed: a small image's share of the
    /// area is too small for its encoding's fixed overhead.
    #[test]
    fn keeps_images_that_fit_together_as_is() {
        let message = format!(
            r#"<img src="{}" /><img src="{}" />"#,
            png_data_url(&noise(64, 64)),
            png_data_url(&gradient(1600, 1200))
        );
        let sent = sent_images(&sanitize_user_message(&LIMITS, &message).unwrap());
        let sizes: Vec<_> = sent.iter().map(|i| (i.width(), i.height())).collect();
        assert_eq!(sizes, [(64, 64), (1600, 1200)]);
    }

    #[test]
    fn sends_images_unchanged_when_unlimited() {
        let limits = MessageLimits { image_message_length: 0, ..LIMITS };
        let message = format!(r#"<img src="{}" />"#, png_data_url(&noise(1600, 1200)));
        assert_eq!(sanitize_user_message(&limits, &message).unwrap(), message);
    }

    #[test]
    fn rejects_images_when_the_server_disallows_html() {
        let limits = MessageLimits { allow_html: false, ..LIMITS };
        let message = format!(r#"hi<img src="{}" />"#, png_data_url(&gradient(40, 30)));
        let error = sanitize_user_message(&limits, &message).unwrap_err();
        assert!(error.to_string().contains("images"), "{error}");
    }

    #[test]
    fn rejects_images_with_no_room_left_by_the_text() {
        let text = "a".repeat(LIMITS.image_message_length - 100);
        let message = format!(r#"{text}<img src="{}" />"#, png_data_url(&noise(64, 64)));
        assert!(sanitize_user_message(&LIMITS, &message).is_err());
    }

    #[test]
    fn sends_text_up_to_message_length() {
        let message = "a".repeat(LIMITS.message_length);
        assert_eq!(sanitize_user_message(&LIMITS, &message).unwrap(), message);
    }

    /// Murmur rejects text over message_length outright, and alongside images
    /// checks the text with some markup of its own added.
    #[test]
    fn rejects_text_too_long_to_send() {
        let text_only = "a".repeat(LIMITS.message_length + 1);
        assert!(sanitize_user_message(&LIMITS, &text_only).is_err());

        let img = format!(r#"<img src="{}" />"#, png_data_url(&noise(64, 64)));
        let with_image = format!("{}{img}", "a".repeat(LIMITS.message_length - 10));
        assert!(sanitize_user_message(&LIMITS, &with_image).is_err());
        let fits = format!("{}{img}", "a".repeat(LIMITS.message_length - 200));
        sanitize_user_message(&LIMITS, &fits).unwrap();
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
        let message = format!(r#"<img src="{}" />"#, png_data_url(&DynamicImage::ImageRgba8(rgba)));
        let sent = sent_images(&sanitize_user_message(&LIMITS, &message).unwrap());
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

        let message = format!(r#"<img src="data:image/jpeg;base64,{}" />"#, STANDARD.encode(&jpeg));
        let sent = sent_images(&sanitize_user_message(&LIMITS, &message).unwrap());
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
        let src = format!("data:image/gif;base64,{}", STANDARD.encode(&gif));
        let sent = sanitize_user_message(&LIMITS, &format!(r#"<img src="{src}" />"#)).unwrap();
        assert!(sent.contains(&src), "{sent:.60}");
    }
}
