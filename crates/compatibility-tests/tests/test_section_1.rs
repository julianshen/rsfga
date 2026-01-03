use anyhow::Result;
use std::process::Command;
use std::thread;
use std::time::Duration;

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
