//! Build script for generating Rust code from protobuf definitions.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Tell Cargo to rerun if proto files change
    println!("cargo:rerun-if-changed=proto/");

    // Configure tonic-build
    tonic_build::configure()
        // Generate server code
        .build_server(true)
        // Generate client code (useful for testing)
        .build_client(true)
        // Don't generate transport-specific code (we'll handle this ourselves)
        .build_transport(false)
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
