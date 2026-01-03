use anyhow::Result;
use std::process::Command;
use std::thread;
use std::time::Duration;
use tonic::transport::Channel;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;

/// Test: Can start OpenFGA via Docker Compose
#[tokio::test]
async fn test_can_start_openfga_via_docker_compose() -> Result<()> {
    // Arrange: Ensure clean state
    stop_docker_compose()?;

    // Act: Start OpenFGA via docker-compose
    start_docker_compose()?;

    // Assert: Verify OpenFGA is running
    let is_running = check_openfga_running().await?;
    assert!(is_running, "OpenFGA should be running after docker-compose up");

    // Cleanup
    stop_docker_compose()?;

    Ok(())
}

/// Start OpenFGA using docker-compose
fn start_docker_compose() -> Result<()> {
    // Get path to compatibility-tests crate root
    let crate_root = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");
    let compose_file = format!("{}/docker-compose.yml", crate_root);

    let output = Command::new("docker-compose")
        .args(&[
            "-f",
            &compose_file,
            "up",
            "-d",
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to start docker-compose: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Wait for services to be healthy
    // OpenFGA needs time to initialize and run migrations
    thread::sleep(Duration::from_secs(20));

    Ok(())
}

/// Stop OpenFGA using docker-compose
fn stop_docker_compose() -> Result<()> {
    // Get path to compatibility-tests crate root
    let crate_root = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");
    let compose_file = format!("{}/docker-compose.yml", crate_root);

    let output = Command::new("docker-compose")
        .args(&[
            "-f",
            &compose_file,
            "down",
            "-v",  // Remove volumes
        ])
        .output()?;

    if !output.status.success() {
        eprintln!(
            "Warning: Failed to stop docker-compose: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Check if OpenFGA is running by checking container status
async fn check_openfga_running() -> Result<bool> {
    let output = Command::new("docker")
        .args(&[
            "ps",
            "--filter",
            "name=openfga",
            "--filter",
            "status=running",
            "--format",
            "{{.Names}}",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let is_running = stdout.contains("openfga");

    Ok(is_running)
}

/// Test: Can connect to OpenFGA HTTP API (health check)
#[tokio::test]
async fn test_can_connect_to_openfga_http_api() -> Result<()> {
    // Arrange: Start OpenFGA
    stop_docker_compose()?;
    start_docker_compose()?;

    // Act: Connect to HTTP health check endpoint
    let client = reqwest::Client::new();
    let response = client
        .get("http://localhost:18080/healthz")
        .send()
        .await?;

    // Assert: Health check returns 200 OK
    assert!(
        response.status().is_success(),
        "Health check should return 2xx status, got: {}",
        response.status()
    );

    // Cleanup
    stop_docker_compose()?;

    Ok(())
}

/// Test: Can connect to OpenFGA gRPC API
#[tokio::test]
async fn test_can_connect_to_openfga_grpc_api() -> Result<()> {
    // Arrange: Start OpenFGA
    stop_docker_compose()?;
    start_docker_compose()?;

    // Act: Connect to gRPC health check endpoint
    let channel = Channel::from_static("http://localhost:18081")
        .connect()
        .await?;

    let mut health_client = HealthClient::new(channel);
    let request = tonic::Request::new(HealthCheckRequest {
        service: "".to_string(), // Empty string checks overall server health
    });

    let response = health_client.check(request).await?;

    // Assert: Health check returns SERVING status
    assert_eq!(
        response.get_ref().status,
        1, // 1 = SERVING in gRPC health check protocol
        "gRPC health check should return SERVING status (1), got: {}",
        response.get_ref().status
    );

    // Cleanup
    stop_docker_compose()?;

    Ok(())
}
