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
        .type_attribute("openfga.v1.AuthorizationModel", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.TypeDefinition", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Relation", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Metadata", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.RelationMetadata", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Userset", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Object", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.UsersetUser", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.TypedWildcard", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Difference", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.TupleToUserset", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Condition", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.ConditionParamTypeRef", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.TypeName", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.ConditionMetadata", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.RelationReference", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Wildcard", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Usersets", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.DirectUserset", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.ObjectRelation", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Oneof attributes - trying to target the synthetic enum by field name path
        .type_attribute("openfga.v1.RelationReference.relation_or_wildcard", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("openfga.v1.Userset.userset", "#[derive(serde::Serialize, serde::Deserialize)]")
        .file_descriptor_set_path(
            std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap())
                .join("openfga_descriptor.bin"),
        )
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
