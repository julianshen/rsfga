//! Build script for generating Rust code from protobuf definitions.

use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Tell Cargo to rerun if proto files change
    println!("cargo:rerun-if-changed=proto/");

    // Output directory for file descriptor set (for gRPC reflection)
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Configure tonic-build
    tonic_build::configure()
        // Generate server code
        .build_server(true)
        // Generate client code (useful for testing)
        .build_client(true)
        // Don't generate transport-specific code (we'll handle this ourselves)
        .build_transport(false)
        // Output file descriptor set for gRPC reflection
        .file_descriptor_set_path(out_dir.join("openfga_descriptor.bin"))
        // Compile the proto files
        .compile(
            &[
                "proto/openfga/v1/openfga.proto",
                "proto/openfga/v1/openfga_service.proto",
            ],
            &["proto/"],
        )?;

    Ok(())
}
