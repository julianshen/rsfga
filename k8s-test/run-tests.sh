#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
REPORT_DIR="$PROJECT_DIR/test-reports"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Create report directory
mkdir -p "$REPORT_DIR"

# Backend to port mapping function (compatible with bash 3.x)
get_port() {
    case "$1" in
        postgres) echo 30081 ;;
        cockroachdb) echo 30082 ;;
        mariadb) echo 30083 ;;
        tidb) echo 30084 ;;
        *) echo "" ;;
    esac
}

BACKENDS="postgres cockroachdb mariadb tidb"

cleanup() {
    log_info "Cleaning up k8s resources..."
    kubectl delete namespace rsfga-test --ignore-not-found=true 2>/dev/null || true
}

deploy_infrastructure() {
    log_info "Deploying test infrastructure to k8s..."

    # Create namespace
    kubectl apply -f "$SCRIPT_DIR/namespace.yaml"

    # Deploy databases
    kubectl apply -f "$SCRIPT_DIR/databases.yaml"

    log_info "Waiting for databases to be ready..."
    kubectl -n rsfga-test wait --for=condition=ready pod -l app=postgres --timeout=120s || true
    kubectl -n rsfga-test wait --for=condition=ready pod -l app=cockroachdb --timeout=120s || true
    kubectl -n rsfga-test wait --for=condition=ready pod -l app=mariadb --timeout=120s || true
    kubectl -n rsfga-test wait --for=condition=ready pod -l app=tidb --timeout=120s || true

    # Give databases a bit more time to fully initialize
    log_info "Giving databases extra time to initialize..."
    sleep 10

    # Deploy RSFGA instances
    kubectl apply -f "$SCRIPT_DIR/rsfga-deployments.yaml"

    log_info "Waiting for RSFGA instances to be ready..."
    for backend in $BACKENDS; do
        log_info "  Waiting for rsfga-$backend..."
        kubectl -n rsfga-test wait --for=condition=ready pod -l app=rsfga-$backend --timeout=180s || {
            log_warn "rsfga-$backend not ready, checking logs..."
            kubectl -n rsfga-test logs -l app=rsfga-$backend --tail=50 || true
        }
    done

    log_info "Checking pod status..."
    kubectl -n rsfga-test get pods
}

check_health() {
    local backend=$1
    local port=$(get_port "$backend")
    local url="http://localhost:$port/health"

    log_info "Checking health of $backend at $url..."

    for i in $(seq 1 30); do
        if curl -sf "$url" > /dev/null 2>&1; then
            log_success "$backend is healthy"
            return 0
        fi
        sleep 2
    done

    log_error "$backend health check failed"
    return 1
}

run_tests_for_backend() {
    local backend=$1
    local port=$(get_port "$backend")
    local report_file="$REPORT_DIR/${backend}_${TIMESTAMP}.txt"
    local json_report="$REPORT_DIR/${backend}_${TIMESTAMP}.json"

    log_info "Running compatibility tests against $backend (port $port)..."

    # Check health first
    if ! check_health "$backend"; then
        echo "SKIPPED: Health check failed" > "$report_file"
        cat > "$json_report" << EOF
{
    "backend": "$backend",
    "timestamp": "$TIMESTAMP",
    "url": "http://localhost:$port",
    "passed": 0,
    "failed": 0,
    "ignored": 0,
    "exit_code": 1,
    "status": "skipped"
}
EOF
        return 1
    fi

    # Run tests
    cd "$PROJECT_DIR"

    set +e
    OPENFGA_URL="http://localhost:$port" cargo test \
        -p compatibility-tests \
        --no-fail-fast \
        -- --test-threads=1 2>&1 | tee "$report_file"

    local exit_code=${PIPESTATUS[0]}
    set -e

    # Parse results
    local passed=$(grep -c "test .* ok$" "$report_file" 2>/dev/null || echo "0")
    local failed=$(grep -c "test .* FAILED$" "$report_file" 2>/dev/null || echo "0")
    local ignored=$(grep -c "test .* ignored$" "$report_file" 2>/dev/null || echo "0")

    # Create JSON summary
    cat > "$json_report" << EOF
{
    "backend": "$backend",
    "timestamp": "$TIMESTAMP",
    "url": "http://localhost:$port",
    "passed": $passed,
    "failed": $failed,
    "ignored": $ignored,
    "exit_code": $exit_code
}
EOF

    if [ $exit_code -eq 0 ]; then
        log_success "$backend: $passed passed, $failed failed, $ignored ignored"
    else
        log_error "$backend: $passed passed, $failed failed, $ignored ignored (exit code: $exit_code)"
    fi

    return $exit_code
}

generate_summary_report() {
    local summary_file="$REPORT_DIR/summary_${TIMESTAMP}.md"

    log_info "Generating summary report..."

    cat > "$summary_file" << EOF
# RSFGA Compatibility Test Report

**Generated:** $(date)
**Test Run ID:** $TIMESTAMP

## Summary

| Backend | Passed | Failed | Ignored | Status |
|---------|--------|--------|---------|--------|
EOF

    local total_passed=0
    local total_failed=0
    local total_ignored=0

    for backend in $BACKENDS; do
        local json_file="$REPORT_DIR/${backend}_${TIMESTAMP}.json"
        if [ -f "$json_file" ]; then
            # Use grep/sed instead of jq for compatibility
            local passed=$(grep '"passed"' "$json_file" | sed 's/[^0-9]//g')
            local failed=$(grep '"failed"' "$json_file" | sed 's/[^0-9]//g')
            local ignored=$(grep '"ignored"' "$json_file" | sed 's/[^0-9]//g')
            local exit_code=$(grep '"exit_code"' "$json_file" | sed 's/[^0-9]//g')

            local status="✅ Pass"
            if [ "$exit_code" != "0" ]; then
                status="❌ Fail"
            fi

            echo "| $backend | $passed | $failed | $ignored | $status |" >> "$summary_file"

            total_passed=$((total_passed + passed))
            total_failed=$((total_failed + failed))
            total_ignored=$((total_ignored + ignored))
        else
            echo "| $backend | - | - | - | ⏭️ Skipped |" >> "$summary_file"
        fi
    done

    cat >> "$summary_file" << EOF
| **Total** | **$total_passed** | **$total_failed** | **$total_ignored** | |

## Test Environment

- **Kubernetes:** $(kubectl version --client -o json 2>/dev/null | grep gitVersion | head -1 | sed 's/.*"gitVersion": "\([^"]*\)".*/\1/' || echo "unknown")
- **Platform:** $(uname -s) $(uname -m)
- **RSFGA Image:** ghcr.io/julianshen/rsfga:latest

## Detailed Reports

EOF

    for backend in $BACKENDS; do
        local log_file="$REPORT_DIR/${backend}_${TIMESTAMP}.txt"
        if [ -f "$log_file" ]; then
            echo "### $backend" >> "$summary_file"
            echo "" >> "$summary_file"
            echo "See: \`${backend}_${TIMESTAMP}.txt\`" >> "$summary_file"
            echo "" >> "$summary_file"

            # Add failed tests if any
            local failed_tests=$(grep "test .* FAILED$" "$log_file" 2>/dev/null || true)
            if [ -n "$failed_tests" ]; then
                echo "**Failed Tests:**" >> "$summary_file"
                echo "\`\`\`" >> "$summary_file"
                echo "$failed_tests" >> "$summary_file"
                echo "\`\`\`" >> "$summary_file"
                echo "" >> "$summary_file"
            fi
        fi
    done

    log_success "Summary report generated: $summary_file"
    echo ""
    echo "=========================================="
    cat "$summary_file"
    echo "=========================================="
}

# Main execution
main() {
    log_info "Starting RSFGA compatibility tests with 4 storage backends"
    log_info "Report directory: $REPORT_DIR"

    # Cleanup any existing deployment
    cleanup

    # Deploy infrastructure
    deploy_infrastructure

    # Run tests for each backend
    local all_passed=true
    for backend in $BACKENDS; do
        if ! run_tests_for_backend "$backend"; then
            all_passed=false
        fi
    done

    # Generate summary
    generate_summary_report

    # Cleanup
    if [ "${KEEP_INFRA:-false}" != "true" ]; then
        cleanup
    else
        log_info "Keeping infrastructure running (KEEP_INFRA=true)"
    fi

    if [ "$all_passed" = true ]; then
        log_success "All tests completed successfully!"
        exit 0
    else
        log_error "Some tests failed. Check reports for details."
        exit 1
    fi
}

# Handle arguments
case "${1:-}" in
    deploy)
        deploy_infrastructure
        ;;
    cleanup)
        cleanup
        ;;
    test)
        shift
        if [ -n "$1" ]; then
            run_tests_for_backend "$1"
        else
            log_error "Please specify a backend: postgres, cockroachdb, mariadb, tidb"
            exit 1
        fi
        ;;
    report)
        generate_summary_report
        ;;
    *)
        main
        ;;
esac
