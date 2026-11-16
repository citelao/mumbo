use base64::{Engine, engine::general_purpose::STANDARD};
use data_url::DataUrl;
use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
use image::{DynamicImage, imageops::FilterType};
use lol_html::errors::RewritingError;
use lol_html::html_content::Element;
use lol_html::{element, HandlerResult, HtmlRewriter, Settings};
use mumble_protocol::state::MumbleState;

/// The largest size an image is displayed at, in CSS/Qt logical pixels;
/// smaller images keep theirs. Keep in sync with MainPage.module.scss.
const MAX_DISPLAY_WIDTH: u32 = 800;
const MAX_DISPLAY_HEIGHT: u32 = 600;
/// Image pixels per displayed pixel, at most, so images are sharp on HiDPI
/// displays. Both mumbo and the official client draw images at their `width`
/// and `height` attributes, which we always set to the display size.
const MAX_PIXEL_DENSITY: u32 = 2;
const JPEG_QUALITY: u8 = 80;

/// When an image must shrink to fit its share of the message, the fraction of
/// that share to aim for (a little under, since the guess is rough), and the
/// least to settle for (each try re-encodes the whole image).
const TARGET_BUDGET_USE: f64 = 0.98;
const MIN_BUDGET_USE: f64 = 0.95;

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

    /// Max total length of a message's image data (its `src` values), given
    /// the length of everything else, or `None` if images are unlimited.
    ///
    /// Murmur rejects messages longer than image_message_length, then checks
    /// only what's left after removing image data against message_length, so
    /// the images can use all the rest. See `Server::isTextAllowed`:
    /// https://github.com/mumble-voip/mumble/blob/ccf405905f8d14ef84f8bc9d8959c6f4b9881b8e/src/murmur/Server.cpp#L2497-L2555
    fn image_budget(&self, other_length: usize) -> Option<usize> {
        if self.image_message_length == 0 {
            return None;
        }
        // Murmur counts UTF-16 code units; bytes are never fewer.
        Some(self.image_message_length.saturating_sub(other_length))
    }
}

/// `img`'s size shrunk to fit within `max_width`×`max_height`, keeping its
/// aspect ratio; never larger than it is.
fn fit(img: &DynamicImage, max_width: u32, max_height: u32) -> (u32, u32) {
    let (width, height) = (img.width() as f64, img.height() as f64);
    let scale = (max_width as f64 / width).min(max_height as f64 / height).min(1.0);
    (((width * scale).round() as u32).max(1), ((height * scale).round() as u32).max(1))
}

fn display_size(img: &DynamicImage) -> (u32, u32) {
    fit(img, MAX_DISPLAY_WIDTH, MAX_DISPLAY_HEIGHT)
}

/// The most pixels `img` is sent with, if there's room.
fn full_size(img: &DynamicImage) -> (u32, u32) {
    fit(img, MAX_DISPLAY_WIDTH * MAX_PIXEL_DENSITY, MAX_DISPLAY_HEIGHT * MAX_PIXEL_DENSITY)
}

/// `img` resized to `size`, or as is if it's already that size.
fn resized(img: &DynamicImage, (width, height): (u32, u32)) -> DynamicImage {
    if (width, height) == (img.width(), img.height()) {
        img.clone()
    } else {
        img.resize_exact(width, height, FilterType::Lanczos3)
    }
}

// fn resize_images_to_fit(
//     images: &[DynamicImage],
//     budget: usize,
// ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
//     // For each image, assign budget of available space proportionally to its
//     // original size. Start with smallest images first, since they compress
//     // less.

//     // Tag each image with index to track original order.
//     let images_with_index = images
//         .iter()
//         .enumerate()
//         .collect::<Vec<_>>();
// }

/// Encodes each image as a data URL, together at most `budget` long.
///
/// Only a lone image is shrunk to fit. Most messages have one, and dividing
/// the budget between several would shrink all of them; sending them
/// separately is better.
// fn encode_to_fit(
//     images: &[DynamicImage],
//     budget: Option<usize>,
// ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
//     let urls = images
//         .iter()
//         .map(|img| smallest_data_url(&resized(img, full_size(img))))
//         .collect::<Result<Vec<_>, _>>()?;
//     let Some(budget) = budget else { return Ok(urls) };
//     if urls.iter().map(String::len).sum::<usize>() <= budget {
//         return Ok(urls);
//     }
//     match images {
//         [image] => Ok(vec![
//             shrink_to_fit(image, &urls[0], budget)?.ok_or("This image is too large to send")?,
//         ]),
//         _ => Err("These images are too large to send together; try sending them separately".into()),
//     }
// }

#[derive(Debug, Clone)]
struct ResizeResult {
    pub success: bool,

    // Best URL. If successful, this is within budget. If not, the closest we got.
    pub best_url: String,
    pub height: u32,
    pub width: u32,
}

// Encode `image` as a data URL at most `budget` long, resizing as needed. Best effort.
fn resize_image_to_fit(
    image: &DynamicImage,
    budget: usize,
) -> Result<ResizeResult, Box<dyn std::error::Error + Send + Sync>> {
    let budget_f = budget as f64;
    let mut best_url = smallest_data_url(image)?;
    let (mut best_height, mut best_width) = (image.height(), image.width());
    let mut best_scale_factor = 1.0;

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
    return Ok(ResizeResult { success, best_url, width: best_width, height: best_height });
}


// /// Encodes `source` as a data URL at most `budget` long, as large as fits
// /// within a few tries, or `None` if none did. `url` is its encoding at full
// /// size, which is too long.
// fn shrink_to_fit(
//     source: &DynamicImage,
//     url: &str,
//     budget: usize,
// ) -> Result<Option<String>, image::ImageError> {
//     let full = full_size(source);
//     let budget_f = budget as f64;

//     // Scales relative to the full size: the largest known to fit, the
//     // smallest known not to, and the one to try next.
//     let (mut fits, mut too_big) = (0.0, 1.0);
//     let mut best = None;
//     let mut scale = 1.0;
//     let mut length = url.len();
//     for _ in 0..8 {
//         // Encoded size is roughly proportional to pixel count, so guess from
//         // the last try. It's only rough (shrinking smooths the image, which
//         // then compresses better), so fall back to bisecting.
//         let guess = scale * (budget_f * TARGET_BUDGET_USE / length as f64).sqrt();
//         scale = if fits < guess && guess < too_big { guess } else { (fits + too_big) / 2.0 };

//         let (width, height) = (full.0 as f64 * scale, full.1 as f64 * scale);
//         if width < 1.0 || height < 1.0 {
//             break;
//         }
//         let attempt = resized(source, (width as u32, height as u32));
//         let url = smallest_data_url(&attempt)?;
//         length = url.len();
//         if length <= budget {
//             if length as f64 >= budget_f * MIN_BUDGET_USE {
//                 return Ok(Some(url));
//             }
//             fits = scale;
//             best = Some(url);
//         } else {
//             too_big = scale;
//         }
//     }
//     Ok(best)
// }

fn smallest_data_url(img: &DynamicImage) -> Result<String, image::ImageError> {
    let mut png = Vec::new();
    img.write_with_encoder(PngEncoder::new(&mut png))?;

    let mut jpeg = Vec::new();
    // JPEG has no alpha channel; transparent areas take their underlying color.
    DynamicImage::ImageRgb8(img.to_rgb8())
        .write_with_encoder(JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY))?;

    Ok(if png.len() <= jpeg.len() {
        format!("data:image/png;base64,{}", STANDARD.encode(&png))
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

struct ImageWithUrl {
    image: DynamicImage,
    url: String,
}

pub fn sanitize_user_message(
    limits: &MessageLimits,
    message: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Two passes, since how much each image can be re-encoded to depends on
    // all of them: first decode them, then replace them in the same order.
    let mut images = vec![];
    let mut image_data_length = 0;
    rewrite(message, |el| {
        let src = el.get_attribute("src").unwrap_or_default();
        if !src.starts_with("data:") {
            // The official client won't load remote images.
            return Err("Only embedded (data: URL) images can be sent".into());
        }
        // Data URLs never contain entities, so this is also the raw length.
        image_data_length += src.len();
        let (body, _fragment) = DataUrl::process(&src)?.decode_to_vec()?;
        // Detects the format from the data itself.
        images.push(ImageWithUrl {
            image: image::load_from_memory(&body)?,
            url: src,
        });
        Ok(())
    })?;

    let text_length = message.len() - image_data_length;
    if text_length > limits.message_length {
        log::warn!("Message exceeds text length limit {}/{} (non-image text)", text_length, limits.message_length);
    }

    // Image space must be the remaining space after accounting for text length.
    let space_for_images = limits.image_message_length.saturating_sub(text_length);

    // Best-effort, simple resize to fit images within the available space for
    // images. Resize from smallest to largest, since small images shrink less.
    //
    // TODO: use visible area instead of URL length for image budgeting.
    let mut resize_order: Vec<usize> = (0..images.len()).collect();
    resize_order.sort_by_key(|&i| images[i].url.len());

    let mut resize_results: Vec<Option<ResizeResult>> = vec![None; images.len()];
    let mut remaining_space = space_for_images;
    for i in resize_order {
        let image = &images[i];
        let proportion = image.url.len() as f64 / space_for_images as f64;
        let budget = (proportion * remaining_space as f64) as usize;
        let resized = resize_image_to_fit(&image.image, budget)?;
        let best_url_len = resized.best_url.len();
        if !resized.success {
            log::warn!("Image could not be resized within budget {}/{}", best_url_len, budget);
        }
        resize_results[i] = Some(resized);
        remaining_space = remaining_space.saturating_sub(best_url_len);
    }

    let mut results = resize_results
        .into_iter()
        .map(|u| u.expect("every image was resized"));

    let output = rewrite(message, |el| {
        let r = results.next().ok_or("Message changed between passes")?;

        // Compute the scale factor to fit the image within the maximum display
        // dimensions, preserving aspect ratio.
        let scale = (MAX_DISPLAY_WIDTH as f64 / r.width as f64)
            .min(MAX_DISPLAY_HEIGHT as f64 / r.height as f64)
            .min(1.0);

        el.set_attribute("src", &r.best_url)?;
        el.set_attribute("width", &(r.width as f64 * scale).to_string())?;
        el.set_attribute("height", &(r.height as f64 * scale).to_string())?;
        Ok(())
    })?;

    // Check message length & warn, but still send.
    if limits.allow_html {
        // We already warned if text was too long.
        if output.len() > limits.image_message_length {
            log::warn!("Message exceeds length limit {}/{} (including images)", output.len(), limits.image_message_length);
        }
    } else {
        if output.len() > limits.message_length {
            log::warn!("Message exceeds length limit {}/{} (HTML not allowed)", output.len(), limits.message_length);
        }
    }

    return Ok(String::from_utf8(output)?);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};
    use std::io::Cursor;

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

    #[test]
    fn keeps_the_display_size_of_shrunk_images() {
        let message = format!(r#"<img src="{}" />"#, png_data_url(&noise(1600, 1200)));
        let sent = sanitize_user_message(&LIMITS, &message).unwrap();
        assert!(sent_images(&sent)[0].width() < 800);
        assert_eq!(displayed_sizes(&sent), [("800".into(), "600".into())]);
    }

    /// Murmur only checks the text left after removing image data against
    /// message_length, so an image can use the rest of image_message_length.
    #[test]
    fn shrinks_incompressible_images_to_fill_the_limit() {
        let message = format!(r#"<img src="{}" />"#, png_data_url(&noise(1600, 1200)));
        let sent = sanitize_user_message(&LIMITS, &message).unwrap();
        let used = sent.len() as f64 / LIMITS.image_message_length as f64;
        assert!((0.95..=1.0).contains(&used), "used {used:.3} of the limit");
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
    fn rejects_several_images_that_dont_fit_together() {
        let src = png_data_url(&noise(800, 600));
        let message = format!(r#"<img src="{src}" /><img src="{src}" />"#);
        let error = sanitize_user_message(&LIMITS, &message).unwrap_err();
        assert!(error.to_string().contains("separately"), "{error}");
    }
}
