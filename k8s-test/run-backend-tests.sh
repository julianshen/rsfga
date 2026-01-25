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

mkdir -p "$REPORT_DIR"

# Backend to port mapping
get_port() {
    case "$1" in
        postgres) echo 30081 ;;
        cockroachdb) echo 30082 ;;
        mariadb) echo 30083 ;;
        tidb) echo 30084 ;;
        *) echo "" ;;
    esac
}

check_health() {
    local backend=$1
    local port=$(get_port "$backend")
    local url="http://localhost:$port/health"

    log_info "Checking health of $backend at $url..."

    for i in $(seq 1 10); do
        if curl -sf "$url" > /dev/null 2>&1; then
            log_success "$backend is healthy"
            return 0
        fi
        sleep 1
    done

    log_error "$backend health check failed"
    return 1
}

run_tests_for_backend() {
    local backend=$1
    local port=$(get_port "$backend")
    local report_file="$REPORT_DIR/${backend}_${TIMESTAMP}.txt"

    log_info "Running compatibility tests against $backend (port $port)..."

    # Check health first
    if ! check_health "$backend"; then
        echo "SKIPPED: Health check failed" > "$report_file"
        return 1
    fi

    # Find all test binaries
    cd "$PROJECT_DIR"

    local total_passed=0
    local total_failed=0
    local all_results=""

    echo "============================================================" > "$report_file"
    echo "RSFGA Compatibility Test Report - $backend" >> "$report_file"
    echo "URL: http://localhost:$port" >> "$report_file"
    echo "Timestamp: $(date)" >> "$report_file"
    echo "============================================================" >> "$report_file"
    echo "" >> "$report_file"

    # Run each test binary
    for test_binary in $(find "$PROJECT_DIR/target/debug/deps" -name 'test_section_*' -type f -perm +111 ! -name '*.o' ! -name '*.d' 2>/dev/null | sort); do
        local test_name=$(basename "$test_binary" | sed 's/-[a-f0-9]*$//')

        log_info "  Running $test_name..."

        # Run tests, excluding common::tests module (which modifies env vars)
        set +e
        local output=$(OPENFGA_URL="http://localhost:$port" "$test_binary" --test-threads=1 --skip common::tests 2>&1)
        local exit_code=$?
        set -e

        echo "" >> "$report_file"
        echo "--- $test_name ---" >> "$report_file"
        echo "$output" >> "$report_file"

        # Parse results
        local passed=$(echo "$output" | grep -E "^test result:" | grep -oE "[0-9]+ passed" | grep -oE "[0-9]+" || echo "0")
        local failed=$(echo "$output" | grep -E "^test result:" | grep -oE "[0-9]+ failed" | grep -oE "[0-9]+" || echo "0")

        if [ -n "$passed" ]; then
            total_passed=$((total_passed + passed))
        fi
        if [ -n "$failed" ]; then
            total_failed=$((total_failed + failed))
        fi

        if [ $exit_code -eq 0 ]; then
            log_success "    $test_name: $passed passed, $failed failed"
        else
            log_error "    $test_name: $passed passed, $failed failed (exit: $exit_code)"
        fi
    done

    echo "" >> "$report_file"
    echo "============================================================" >> "$report_file"
    echo "TOTAL: $total_passed passed, $total_failed failed" >> "$report_file"
    echo "============================================================" >> "$report_file"

    log_info "$backend total: $total_passed passed, $total_failed failed"

    if [ $total_failed -eq 0 ]; then
        return 0
    else
        return 1
    fi
}

# Main
main() {
    local backend="${1:-postgres}"

    log_info "Starting compatibility tests for $backend"
    log_info "Report directory: $REPORT_DIR"

    run_tests_for_backend "$backend"

    log_success "Report saved to: $REPORT_DIR/${backend}_${TIMESTAMP}.txt"
}

main "$@"
