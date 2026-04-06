/// Minimal build script for the sonic-sdn crate.
///
/// The gNMI, gNOI, and P4Runtime protobuf message types are modeled as plain
/// Rust structs rather than generated from `.proto` files via `tonic-build`.
/// This avoids a build-time dependency on protoc and the proto definitions.
fn main() {
    // No codegen required. Proto types are hand-modeled in the source.
}
