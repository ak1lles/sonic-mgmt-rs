/// Build script that compiles gNMI, gNOI, and P4Runtime proto definitions
/// into Rust types and tonic service clients.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = [
        "proto/gnmi.proto",
        "proto/gnoi.proto",
        "proto/gnoi_types.proto",
        "proto/p4runtime.proto",
        "proto/p4info.proto",
        "proto/google/rpc/status.proto",
    ];

    tonic_build::configure()
        .build_server(false)
        .compile_protos(&proto_files, &["proto"])?;

    Ok(())
}
