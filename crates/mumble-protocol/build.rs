fn main() -> std::io::Result<()> {
    // Use a vendored protoc so builds don't depend on a system install.
    let protoc = protoc_bin_vendored::protoc_bin_path().map_err(std::io::Error::other)?;
    prost_build::Config::new()
        .protoc_executable(protoc)
        .compile_protos(
            &["src/proto/Mumble.proto", "src/proto/MumbleUDP.proto"],
            &["src/proto/"],
        )
}