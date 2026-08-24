fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.file_descriptor_set_path(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is unavailable")?)
            .join("termirust.host.v1.bin"),
    );
    config.compile_protos(&["proto/host.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/host.proto");
    Ok(())
}
