//! Sends variants of an image message to a live Mumble server and reports
//! which ones it accepts.
//!
//! Murmur's `Server::isTextAllowed` lets a message exceed `textmessagelength`
//! (default 5000) only if, after stripping `<img src>`s, it fits. It does that
//! stripping with `QXmlStreamReader`, so the message must be well-formed XML;
//! if not, the server replies `PermissionDenied { type: TextTooLong }` no
//! matter how short the text is.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use mumble_protocol::ServerToClientMessage;
use mumble_protocol::client::{ConnectionOptions, connect};
use mumble_protocol::raw::permission_denied::DenyType;
use std::time::Duration;

fn options(username: &str) -> ConnectionOptions {
    // Defaults match dev/mumble-server/compose.yaml.
    ConnectionOptions {
        server: std::env::var("MUMBLE_SERVER").unwrap_or("localhost".into()),
        port: Some(
            std::env::var("MUMBLE_PORT")
                .map(|p| p.parse().expect("MUMBLE_PORT must be a port number"))
                .unwrap_or(64738),
        ),
        password: Some(std::env::var("MUMBLE_PASSWORD").unwrap_or("test".into())),
        username: Some(username.into()),
    }
}

/// A `size`×`size` smiley as an uncompressed 24-bit BMP. Uncompressed so a
/// modest size reliably lands over the text limit; BMP so no encoder is needed.
fn smiley_bmp(size: u32) -> Vec<u8> {
    const WHITE: [u8; 3] = [255, 255, 255];
    const YELLOW: [u8; 3] = [0, 210, 255]; // BGR
    const BLACK: [u8; 3] = [0, 0, 0];

    let pixel = |x: u32, y: u32| -> [u8; 3] {
        // Normalized, with y pointing down.
        let (px, py) = ((x as f32 + 0.5) / size as f32, (y as f32 + 0.5) / size as f32);
        let dist = |cx: f32, cy: f32| ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        let from_center = dist(0.5, 0.5);
        if from_center > 0.47 {
            WHITE
        } else if from_center > 0.43
            || dist(0.35, 0.38) < 0.07
            || dist(0.65, 0.38) < 0.07
            || (py > 0.55 && (0.24..0.31).contains(&from_center))
        {
            BLACK
        } else {
            YELLOW
        }
    };

    let row_len = (size * 3).div_ceil(4) * 4;
    let data_len = row_len * size;
    let mut bmp = Vec::with_capacity((54 + data_len) as usize);
    // BITMAPFILEHEADER
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(54 + data_len).to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(size as i32).to_le_bytes());
    bmp.extend_from_slice(&(size as i32).to_le_bytes()); // positive: rows bottom-up
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes()); // uncompressed
    bmp.extend_from_slice(&data_len.to_le_bytes());
    bmp.extend_from_slice(&2835i32.to_le_bytes()); // 72 DPI
    bmp.extend_from_slice(&2835i32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..size).rev() {
        let start = bmp.len();
        for x in 0..size {
            bmp.extend_from_slice(&pixel(x, y));
        }
        bmp.resize(start + row_len as usize, 0);
    }
    bmp
}

fn smiley_src(size: u32) -> String {
    format!("data:image/bmp;base64,{}", BASE64.encode(smiley_bmp(size)))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut sender = connect(options("text-message-tester-sender"))
        .await
        .expect("sender should connect");
    // A second user confirms the message was actually delivered, since the
    // server never acknowledges accepted messages.
    let mut listener = connect(options("text-message-tester-listener"))
        .await
        .expect("listener should connect");

    // ~16 KB: over the 5000-char text limit, well under the 128 KiB image limit.
    let src = smiley_src(64);
    let html_img = format!(r#"<img src="{src}">"#);
    let xhtml_img = format!(r#"<img src="{src}" />"#);

    // (description, message, expected to be accepted)
    let cases: Vec<(&str, String, bool)> = vec![
        ("short <img> (skips the XML check)", format!(r#"<img src="{}">"#, smiley_src(8)), true),
        ("<img ...>", html_img.clone(), false),
        ("<img ... />", xhtml_img.clone(), true),
        ("hi<br><img ... />", format!("hi<br>{xhtml_img}"), false),
        ("hi<br /><img ... />", format!("hi<br />{xhtml_img}"), true),
        ("hi&nbsp;<img ... />", format!("hi&nbsp;{xhtml_img}"), false),
        ("hi&#160;<img ... />", format!("hi&#160;{xhtml_img}"), true),
        ("a&amp;b<img ... />", format!("a&amp;b{xhtml_img}"), true),
    ];

    let mut failures = 0;
    for (description, message, expect_accepted) in cases {
        while sender.debug.try_recv().is_ok() {}
        while listener.debug.try_recv().is_ok() {}

        let length = message.len();
        sender
            .client
            .send_channel_message(message, Some(0))
            .expect("send should succeed");
        // Waits out the server's reply, and Murmur's text-message rate limit.
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let mut denied = None;
        while let Ok(m) = sender.debug.try_recv() {
            if let ServerToClientMessage::PermissionDenied(pd) = m {
                denied = Some(
                    pd.r#type
                        .and_then(|t| DenyType::try_from(t).ok())
                        .map_or("unknown", |t| t.as_str_name()),
                );
            }
        }
        let mut delivered = false;
        while let Ok(m) = listener.debug.try_recv() {
            if matches!(m, ServerToClientMessage::TextMessage(_)) {
                delivered = true;
            }
        }

        let accepted = denied.is_none() && delivered;
        let ok = accepted == expect_accepted;
        if !ok {
            failures += 1;
        }
        println!(
            "{} {description:<36} len={length:<6} expect: {:<9} got: {}",
            if ok { "PASS" } else { "FAIL" },
            if expect_accepted { "delivered" } else { "denied" },
            match (denied, delivered) {
                (Some(t), _) => format!("denied: {t}"),
                (None, true) => "delivered".into(),
                (None, false) => "not delivered, no denial".into(),
            },
        );
    }

    sender.client.disconnect();
    listener.client.disconnect();
    if failures > 0 {
        std::process::exit(1);
    }
}
