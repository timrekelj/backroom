use std::path::PathBuf;
use std::process::Command;

// `connectrpc-build` shells out to `protoc` by default; we already use `buf`
// for the protos, so build the descriptor set with it and feed that in.
fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let descriptor = out_dir.join("backroom.binpb");

    let status = Command::new("buf")
        .current_dir("..")
        .args(["build", "--as-file-descriptor-set", "-o"])
        .arg(&descriptor)
        .status()
        .expect("failed to run `buf` (is buf installed?)");
    assert!(status.success(), "`buf build` failed");

    connectrpc_build::Config::new()
        .descriptor_set(&descriptor)
        .files(&[
            "backroom/v1/auth_service.proto",
            "backroom/v1/gateway_service.proto",
            "backroom/v1/guild_service.proto",
            "backroom/v1/message_service.proto",
            "backroom/v1/types.proto",
        ])
        .include_file("_connectrpc.rs")
        .compile()
        .unwrap();

    println!("cargo:rerun-if-changed=../proto");
    println!("cargo:rerun-if-changed=../buf.yaml");
}
