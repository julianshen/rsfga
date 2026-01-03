use anyhow::Result;
use std::process::Command;
use std::thread;
use std::time::Duration;
use tonic::transport::Channel;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;
use uuid::Uuid;

/// gRPC health check status codes
const GRPC_HEALTH_STATUS_SERVING: i32 = 1;

/// Test: Can start OpenFGA via Docker Compose
#[tokio::test]
async fn test_can_start_openfga_via_docker_compose() -> Result<()> {
    // Arrange: Ensure clean state
    stop_docker_compose()?;

    // Act: Start OpenFGA via docker-compose
    start_docker_compose()?;

    // Assert: Verify OpenFGA is running
    let is_running = check_openfga_running()?;
    assert!(
        is_running,
        "OpenFGA should be running after docker-compose up"
    );

    // Cleanup
    stop_docker_compose()?;

    Ok(())
}

/// Start OpenFGA using docker-compose
fn start_docker_compose() -> Result<()> {
    // Get path to compatibility-tests crate root
    let crate_root = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let compose_file = format!("{}/docker-compose.yml", crate_root);

    let output = Command::new("docker-compose")
        .args(&["-f", &compose_file, "up", "-d"])
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
    let crate_root = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let compose_file = format!("{}/docker-compose.yml", crate_root);

    let output = Command::new("docker-compose")
        .args(&[
            "-f",
            &compose_file,
            "down",
            "-v", // Remove volumes
        ])
        .output()?;

    if !output.status.success() {
        eprintln!(
            "Warning: Failed to stop docker-compose: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Wait for containers to actually stop (idempotent - safe to call multiple times)
    // Docker-compose down is asynchronous, so we need to wait for containers to be gone
    for _ in 0..10 {
        let check_output = Command::new("docker-compose")
            .args(&[
                "-f",
                &compose_file,
                "ps",
                "-q", // Quiet mode - only IDs
            ])
            .output()?;

        let stdout = String::from_utf8_lossy(&check_output.stdout);
        if stdout.trim().is_empty() {
            // No containers from our compose project - we're done
            return Ok(());
        }

        // Containers still stopping, wait a bit
        thread::sleep(Duration::from_secs(1));
    }

    Ok(())
}

/// Check if OpenFGA is running by checking container status
fn check_openfga_running() -> Result<bool> {
    // Get path to compatibility-tests crate root
    let crate_root = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let compose_file = format!("{}/docker-compose.yml", crate_root);

    let output = Command::new("docker-compose")
        .args(&[
            "-f",
            &compose_file,
            "ps",
            "-q", // Quiet mode - only IDs
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let is_running = !stdout.trim().is_empty();

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
    let response = client.get("http://localhost:18080/healthz").send().await?;

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
        GRPC_HEALTH_STATUS_SERVING,
        "gRPC health check should return SERVING status, got: {}",
        response.get_ref().status
    );

    // Cleanup
    stop_docker_compose()?;

    Ok(())
}

/// Test: Can create a store via API
#[tokio::test]
async fn test_can_create_store_via_api() -> Result<()> {
    // Arrange: Start OpenFGA
    stop_docker_compose()?;
    start_docker_compose()?;

    // Act: Create a store via HTTP API
    let client = reqwest::Client::new();
    let store_name = format!("test-store-{}", Uuid::new_v4());

    let create_request = serde_json::json!({
        "name": store_name
    });

    let response = client
        .post("http://localhost:18080/stores")
        .json(&create_request)
        .send()
        .await?;

    // Assert: Store creation returns 201 Created
    assert!(
        response.status().is_success(),
        "Store creation should return 2xx status, got: {}",
        response.status()
    );

    let response_body: serde_json::Value = response.json().await?;

    // Verify response contains store ID
    assert!(
        response_body.get("id").is_some(),
        "Response should contain store 'id' field"
    );

    // Verify response contains the name we provided
    assert_eq!(
        response_body.get("name").and_then(|v| v.as_str()),
        Some(store_name.as_str()),
        "Response should contain the store name we provided"
    );

    // Cleanup
    stop_docker_compose()?;

    Ok(())
}

/// Test: Can clean up test stores after tests
#[tokio::test]
async fn test_can_clean_up_test_stores() -> Result<()> {
    // Arrange: Start OpenFGA and create a test store
    stop_docker_compose()?;
    start_docker_compose()?;

    let client = reqwest::Client::new();
    let store_name = format!("test-store-{}", Uuid::new_v4());

    let create_request = serde_json::json!({
        "name": store_name
    });

    let create_response = client
        .post("http://localhost:18080/stores")
        .json(&create_request)
        .send()
        .await?;

    assert!(create_response.status().is_success());
    let store_data: serde_json::Value = create_response.json().await?;
    let store_id = store_data
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Store ID not found in response"))?;

    // Act: Delete the store
    let delete_response = client
        .delete(format!("http://localhost:18080/stores/{}", store_id))
        .send()
        .await?;

    // Assert: Deletion returns success (204 No Content)
    assert!(
        delete_response.status().is_success(),
        "Store deletion should return 2xx status, got: {}",
        delete_response.status()
    );

    // Verify: Store no longer exists (GET should return 404)
    let get_response = client
        .get(format!("http://localhost:18080/stores/{}", store_id))
        .send()
        .await?;

    assert_eq!(
        get_response.status(),
        reqwest::StatusCode::NOT_FOUND,
        "Deleted store should return 404, got: {}",
        get_response.status()
    );

    // Cleanup
    stop_docker_compose()?;

    Ok(())
}

/// Test: Test environment teardown is idempotent
#[tokio::test]
async fn test_environment_teardown_is_idempotent() -> Result<()> {
    // Arrange: Start OpenFGA
    stop_docker_compose()?; // Clean state first
    start_docker_compose()?;

    // Act: Stop docker-compose multiple times
    stop_docker_compose()?; // First stop - should succeed
    stop_docker_compose()?; // Second stop - should also succeed (idempotent)
    stop_docker_compose()?; // Third stop - should still succeed

    // Assert: Verify environment is actually stopped
    let is_running = check_openfga_running()?;
    assert!(!is_running, "OpenFGA should not be running after teardown");

    // Final cleanup (just to be thorough)
    stop_docker_compose()?;

    Ok(())
}
