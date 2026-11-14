# Text message tester

## Human summary

A simple tester to verify the hypothesis that invalid XML causes long messages not to be delivered.

Result: indeed. Long messages must be valid XML to pass:

- `<img ...>` fails, `<img ... />` succeeds.
- `<br>` fails, `<br />` succeeds.

This is because in [`Server::isTextAllowed`](https://github.com/mumble-voip/mumble/blob/abb803cf0ec2d0dbb45702290f4748cfbe622fd5/src/murmur/Server.cpp#L2502) the Mumble server reparses any HTML-like messages `m` where `message_length < m < image_message_length`, using `QXmlStreamReader`, which fails on invalid XML. The server will reply with `TextTooLong`.

This also means that long messages will fail with HTML entities like `&nbsp;`; such messages need to be rewritten into XML (`&#160;`) before sending.

## Summary

Checks which text-message HTML a live Mumble server accepts.

Messages over the server's text limit (5000 chars by default) are only allowed
if they contain images and are well-formed XML: `<img ... />`, `<br />`, and no
named entities like `&nbsp;`. Otherwise Murmur replies `TextTooLong`, even when
the text itself is short. See `Server::isTextAllowed` in Mumble's
`src/murmur/Server.cpp`.

## Usage

```sh
npm run mumble:up
cargo run -p text-message-tester
```

Set `MUMBLE_SERVER`, `MUMBLE_PORT`, or `MUMBLE_PASSWORD` to test another server.
