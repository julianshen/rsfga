#!/bin/bash

# RSFGA Load Test Suite Runner
#
# Usage:
#   ./run-suite.sh [scenario] [options]
#
# Scenarios:
#   all          - Run all scenarios
#   check        - Run all check scenarios
#   check-direct - Run direct check scenario
#   check-computed - Run computed check scenario
#   check-deep   - Run deep hierarchy check scenario
#   batch-check  - Run batch check scenario
#   list-objects - Run list objects scenario
#   list-users   - Run list users scenario
#   expand       - Run expand scenario
#   write        - Run write scenario
#   mixed        - Run mixed workload scenario
#
# Options:
#   --url URL         - RSFGA server URL (default: http://localhost:8080)
#   --output DIR      - Output directory for results (default: ../reports)
#   --influxdb URL    - InfluxDB URL for metrics (optional)
#   --duration DUR    - Override default duration
#   --vus NUM         - Override default VU count
#   --dry-run         - Print commands without executing

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K6_DIR="$SCRIPT_DIR/../k6"
REPORTS_DIR="$SCRIPT_DIR/../reports"

# Default configuration
RSFGA_URL="${RSFGA_URL:-http://localhost:8080}"
INFLUXDB_URL=""
DURATION=""
VUS=""
DRY_RUN=false

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

usage() {
    echo "Usage: $0 [scenario] [options]"
    echo ""
    echo "Scenarios:"
    echo "  all              Run all scenarios"
    echo "  check            Run all check scenarios"
    echo "  check-direct     Run direct check scenario"
    echo "  check-computed   Run computed check scenario"
    echo "  check-deep       Run deep hierarchy check scenario"
    echo "  batch-check      Run batch check scenario"
    echo "  list-objects     Run list objects scenario"
    echo "  list-users       Run list users scenario"
    echo "  expand           Run expand scenario"
    echo "  write            Run write scenario"
    echo "  mixed            Run mixed workload scenario"
    echo ""
    echo "Options:"
    echo "  --url URL        RSFGA server URL (default: http://localhost:8080)"
    echo "  --output DIR     Output directory for results"
    echo "  --influxdb URL   InfluxDB URL for Grafana integration"
    echo "  --duration DUR   Override default duration (e.g., '5m')"
    echo "  --vus NUM        Override default VU count"
    echo "  --dry-run        Print commands without executing"
    echo "  -h, --help       Show this help message"
    exit 0
}

check_k6() {
    if ! command -v k6 &> /dev/null; then
        log_error "k6 is not installed. Please install it first:"
        echo ""
        echo "  macOS:   brew install k6"
        echo "  Linux:   https://k6.io/docs/get-started/installation/"
        echo "  Docker:  docker pull grafana/k6"
        exit 1
    fi
    log_info "k6 version: $(k6 version)"
}

check_server() {
    log_info "Checking RSFGA server at $RSFGA_URL..."
    if ! curl -s -o /dev/null -w "%{http_code}" "$RSFGA_URL/health" | grep -q "200"; then
        log_warning "Server health check failed. Continuing anyway..."
    else
        log_success "Server is healthy"
    fi
}

run_scenario() {
    local name="$1"
    local script="$2"
    local timestamp=$(date +%Y%m%d_%H%M%S)
    local output_file="$REPORTS_DIR/${name}_${timestamp}.json"

    log_info "Running scenario: $name"

    # Build k6 command
    local cmd="k6 run"

    # Add environment variables
    cmd="$cmd -e RSFGA_URL=$RSFGA_URL"

    # Add optional overrides
    if [ -n "$DURATION" ]; then
        cmd="$cmd --duration $DURATION"
    fi

    if [ -n "$VUS" ]; then
        cmd="$cmd --vus $VUS"
    fi

    # Add output options
    cmd="$cmd --out json=$output_file"

    if [ -n "$INFLUXDB_URL" ]; then
        cmd="$cmd --out influxdb=$INFLUXDB_URL"
    fi

    # Add script path
    cmd="$cmd $K6_DIR/scenarios/$script"

    if [ "$DRY_RUN" = true ]; then
        echo "Would run: $cmd"
        return 0
    fi

    echo "Command: $cmd"
    echo ""

    # Run k6
    if eval "$cmd"; then
        log_success "Scenario $name completed successfully"
        log_info "Results saved to: $output_file"
    else
        log_error "Scenario $name failed"
        return 1
    fi

    echo ""
}

# Parse arguments
SCENARIO=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            usage
            ;;
        --url)
            RSFGA_URL="$2"
            shift 2
            ;;
        --output)
            REPORTS_DIR="$2"
            shift 2
            ;;
        --influxdb)
            INFLUXDB_URL="$2"
            shift 2
            ;;
        --duration)
            DURATION="$2"
            shift 2
            ;;
        --vus)
            VUS="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        *)
            if [ -z "$SCENARIO" ]; then
                SCENARIO="$1"
            else
                log_error "Unknown argument: $1"
                exit 1
            fi
            shift
            ;;
    esac
done

# Default scenario
SCENARIO="${SCENARIO:-all}"

# Create reports directory
mkdir -p "$REPORTS_DIR"

# Pre-flight checks
check_k6
check_server

log_info "Configuration:"
echo "  Server URL: $RSFGA_URL"
echo "  Reports: $REPORTS_DIR"
[ -n "$INFLUXDB_URL" ] && echo "  InfluxDB: $INFLUXDB_URL"
[ -n "$DURATION" ] && echo "  Duration: $DURATION"
[ -n "$VUS" ] && echo "  VUs: $VUS"
echo ""

# Run scenarios
case $SCENARIO in
    all)
        run_scenario "check-direct" "check-direct.js"
        run_scenario "check-computed" "check-computed.js"
        run_scenario "check-deep-hierarchy" "check-deep-hierarchy.js"
        run_scenario "batch-check" "batch-check.js"
        run_scenario "list-objects" "list-objects.js"
        run_scenario "list-users" "list-users.js"
        run_scenario "expand" "expand.js"
        run_scenario "write-tuples" "write-tuples.js"
        run_scenario "mixed-workload" "mixed-workload.js"
        ;;
    check)
        run_scenario "check-direct" "check-direct.js"
        run_scenario "check-computed" "check-computed.js"
        run_scenario "check-deep-hierarchy" "check-deep-hierarchy.js"
        ;;
    check-direct)
        run_scenario "check-direct" "check-direct.js"
        ;;
    check-computed)
        run_scenario "check-computed" "check-computed.js"
        ;;
    check-deep)
        run_scenario "check-deep-hierarchy" "check-deep-hierarchy.js"
        ;;
    batch-check)
        run_scenario "batch-check" "batch-check.js"
        ;;
    list-objects)
        run_scenario "list-objects" "list-objects.js"
        ;;
    list-users)
        run_scenario "list-users" "list-users.js"
        ;;
    expand)
        run_scenario "expand" "expand.js"
        ;;
    write)
        run_scenario "write-tuples" "write-tuples.js"
        ;;
    mixed)
        run_scenario "mixed-workload" "mixed-workload.js"
        ;;
    *)
        log_error "Unknown scenario: $SCENARIO"
        usage
        ;;
esac

log_success "Load test suite completed!"
echo ""
echo "Results are available in: $REPORTS_DIR"
