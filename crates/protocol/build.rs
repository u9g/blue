use std::path::PathBuf;

const PROTOS: &[&str] = &[
    "blue/events/v1/events.proto",
    "blue/bridge/v1/bridge.proto",
    "blue/runner/v1/runner.proto",
    "blue/client/v1/client.proto",
    "blue/auth/v1/auth.proto",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    println!("cargo:rerun-if-changed={}", proto_root.display());

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);

    let protos: Vec<PathBuf> = PROTOS.iter().map(|p| proto_root.join(p)).collect();
    let includes = [proto_root, protoc_bin_vendored::include_path()?];
    tonic_prost_build::configure().compile_with_config(config, &protos, &includes)?;
    Ok(())
}
