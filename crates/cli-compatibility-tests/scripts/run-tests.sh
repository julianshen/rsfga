#!/bin/bash
#
# CLI Compatibility Test Runner
#
# Runs all .fga.yaml tests against RSFGA (or OpenFGA) using the fga CLI.
#
# Usage:
#   ./run-tests.sh                    # Run all tests against localhost:8080
#   FGA_API_URL=http://localhost:8080 ./run-tests.sh
#   ./run-tests.sh --verbose          # Show detailed output
#   ./run-tests.sh tests/01-*.yaml    # Run specific test files
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TESTS_DIR="${SCRIPT_DIR}/../tests"
FGA_API_URL="${FGA_API_URL:-http://localhost:8080}"
VERBOSE="${VERBOSE:-false}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Counters
TOTAL=0
PASSED=0
FAILED=0
SKIPPED=0

# Sanitize string for use in store names (alphanumeric, dash, underscore only)
sanitize_name() {
    echo "$1" | tr -cd '[:alnum:]-_'
}

# Check if fga CLI is installed
if ! command -v fga &> /dev/null; then
    echo -e "${RED}Error: fga CLI not found${NC}"
    echo "Install it with: brew install openfga/tap/fga"
    echo "Or: go install github.com/openfga/cli/cmd/fga@latest"
    exit 1
fi

# Check if jq is installed
if ! command -v jq &> /dev/null; then
    echo -e "${RED}Error: jq not found${NC}"
    echo "Install it with: brew install jq"
    exit 1
fi

# Check if server is running
echo -e "${BLUE}Checking connection to ${FGA_API_URL}...${NC}"
if ! curl -sf "${FGA_API_URL}/health" > /dev/null 2>&1; then
    echo -e "${RED}Error: Cannot connect to ${FGA_API_URL}${NC}"
    echo "Make sure RSFGA or OpenFGA is running."
    exit 1
fi
echo -e "${GREEN}Server is healthy${NC}"
echo

# Parse arguments
TEST_FILES=()
for arg in "$@"; do
    case "$arg" in
        --verbose|-v)
            VERBOSE="true"
            ;;
        --help|-h)
            echo "Usage: $0 [options] [test-files...]"
            echo ""
            echo "Options:"
            echo "  --verbose, -v    Show detailed test output"
            echo "  --help, -h       Show this help message"
            echo ""
            echo "Environment variables:"
            echo "  FGA_API_URL      API endpoint (default: http://localhost:8080)"
            echo ""
            echo "Examples:"
            echo "  $0                           # Run all tests"
            echo "  $0 tests/01-*.yaml           # Run specific tests"
            echo "  FGA_API_URL=http://localhost:8080 $0"
            exit 0
            ;;
        *)
            TEST_FILES+=("$arg")
            ;;
    esac
done

# If no specific files given, run all tests
if [ ${#TEST_FILES[@]} -eq 0 ]; then
    TEST_FILES=("${TESTS_DIR}"/*.fga.yaml)
fi

echo -e "${BLUE}============================================${NC}"
echo -e "${BLUE}  CLI Compatibility Test Suite${NC}"
echo -e "${BLUE}============================================${NC}"
echo -e "API URL: ${FGA_API_URL}"
echo -e "Test files: ${#TEST_FILES[@]}"
echo

# Run each test file
for test_file in "${TEST_FILES[@]}"; do
    if [ ! -f "$test_file" ]; then
        echo -e "${YELLOW}SKIP${NC} ${test_file} (file not found)"
        ((SKIPPED++))
        continue
    fi

    test_name=$(basename "$test_file" .fga.yaml)
    safe_name=$(sanitize_name "$test_name")
    ((TOTAL++))

    echo -n "Running ${test_name}... "

    # Create a temporary store for this test (use sanitized name to prevent injection)
    store_result=$(fga store create --api-url "${FGA_API_URL}" --name "cli-test-${safe_name}-$$" 2>&1) || {
        echo -e "${RED}FAIL${NC} (store creation failed)"
        if [ "$VERBOSE" = "true" ]; then
            echo "  Error: ${store_result}"
        fi
        ((FAILED++))
        continue
    }

    # Extract store ID from JSON output using jq
    # JSON format: {"store":{"id":"01KFZZSBM316YHPWRH0RHE64SD",...}}
    store_id=$(echo "$store_result" | jq -r '.store.id // empty') || {
        echo -e "${RED}FAIL${NC} (could not extract store ID)"
        if [ "$VERBOSE" = "true" ]; then
            echo "  Output: ${store_result}"
        fi
        ((FAILED++))
        continue
    }

    if [ -z "$store_id" ]; then
        echo -e "${RED}FAIL${NC} (empty store ID)"
        if [ "$VERBOSE" = "true" ]; then
            echo "  Output: ${store_result}"
        fi
        ((FAILED++))
        continue
    fi

    # Run the test (reset exit code before each test)
    test_exit_code=0
    test_output=$(fga model test \
        --api-url "${FGA_API_URL}" \
        --store-id "${store_id}" \
        --tests "${test_file}" 2>&1) || test_exit_code=$?

    # Cleanup: delete the store (log errors in verbose mode)
    if ! cleanup_output=$(fga store delete --api-url "${FGA_API_URL}" --store-id "${store_id}" 2>&1); then
        if [ "$VERBOSE" = "true" ]; then
            echo "  Warning: Failed to cleanup store ${store_id}: ${cleanup_output}" >&2
        fi
    fi

    # Check result
    if [ $test_exit_code -eq 0 ]; then
        echo -e "${GREEN}PASS${NC}"
        ((PASSED++))
        if [ "$VERBOSE" = "true" ]; then
            echo "$test_output" | sed 's/^/  /'
        fi
    else
        echo -e "${RED}FAIL${NC}"
        ((FAILED++))
        echo "$test_output" | sed 's/^/  /'
    fi
done

echo
echo -e "${BLUE}============================================${NC}"
echo -e "${BLUE}  Test Results${NC}"
echo -e "${BLUE}============================================${NC}"
echo -e "Total:   ${TOTAL}"
echo -e "Passed:  ${GREEN}${PASSED}${NC}"
echo -e "Failed:  ${RED}${FAILED}${NC}"
echo -e "Skipped: ${YELLOW}${SKIPPED}${NC}"
echo

if [ $FAILED -gt 0 ]; then
    echo -e "${RED}Some tests failed!${NC}"
    exit 1
else
    echo -e "${GREEN}All tests passed!${NC}"
    exit 0
fi
