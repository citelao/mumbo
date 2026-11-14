use base64::{Engine, engine::general_purpose::STANDARD};
use data_url::DataUrl;
use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
use lol_html::{element, HtmlRewriter, Settings};
use mumble_protocol::state::MumbleState;

pub fn sanitize_user_message(state: &MumbleState, message: &str) -> Result<String, Box<dyn std::error::Error>> {
    // TODO:
    //
    // - parse html
    // - re-encode base64 images
    if !state.allow_html {
        // TODO: sanitize message length
    }

    let mut output = vec![];
    let mut rewriter = HtmlRewriter::new(
        Settings::new()
            .append_element_content_handler(element!("img", |el| {
                let src = el.get_attribute("src").unwrap_or_default();
                if src.starts_with("data:") {
                    let parsed = DataUrl::process(&src)?;
                    let (body, _frag) = parsed.decode_to_vec()?;

                    // Automatically decodes type.
                    let img = image::load_from_memory(&body)?;

                    const MAX_WIDTH: u32 = 300;
                    const MAX_HEIGHT: u32 = 200;
                    let resized = img.resize(MAX_WIDTH, MAX_HEIGHT, image::imageops::FilterType::Lanczos3);

                    let mut resized_png_buf = Vec::new();
                    resized.write_with_encoder(PngEncoder::new(&mut resized_png_buf))?;

                    let mut resized_jpeg_buf = Vec::new();
                    resized.write_with_encoder(JpegEncoder::new(&mut resized_jpeg_buf))?;

                    // Choose the smaller of the two data URLs
                    let chosen_data_url = if resized_png_buf.len() <= resized_jpeg_buf.len() {
                        format!("data:image/png;base64,{}", STANDARD.encode(&resized_png_buf))
                    } else {
                        format!("data:image/jpeg;base64,{}", STANDARD.encode(&resized_jpeg_buf))
                    };

                    el.set_attribute("src", &chosen_data_url.to_string())?;
                } else {
                    // Throw an error for non-base64 images
                    return Err("Non-base64 images are not allowed".into());
                }
                Ok(())
            })),
        |c: &[u8]| output.extend_from_slice(c)
    );

    rewriter.write(message.as_bytes())?;
    rewriter.end()?;

    // Check message length & warn
    // TODO: redo resizing with smaller images as necessary.
    if let Some(max_len) = state.image_message_length {
        if output.len() > max_len.try_into().unwrap_or(usize::MAX) {
            log::warn!("Warning: message length exceeds {}/{}", output.len(), max_len);
        } else {
            log::info!("Message length within limit: {}/{}", output.len(), max_len);
        }
    } else {
        log::warn!("Warning: message length limit not set");
    }

    return Ok(String::from_utf8(output)?);
}