#!/bin/bash
#
# Compare OpenFGA vs RSFGA CLI Compatibility
#
# Runs all tests against both implementations and compares results.
#
# Usage:
#   ./compare-implementations.sh
#
# Prerequisites:
#   - OpenFGA running on localhost:18080 (or set OPENFGA_URL)
#   - RSFGA running on localhost:8080 (or set RSFGA_URL)
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TESTS_DIR="${SCRIPT_DIR}/../tests"

OPENFGA_URL="${OPENFGA_URL:-http://localhost:18080}"
RSFGA_URL="${RSFGA_URL:-http://localhost:8080}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Check fga CLI
if ! command -v fga &> /dev/null; then
    echo -e "${RED}Error: fga CLI not found${NC}"
    exit 1
fi

echo -e "${CYAN}============================================${NC}"
echo -e "${CYAN}  OpenFGA vs RSFGA Compatibility Comparison${NC}"
echo -e "${CYAN}============================================${NC}"
echo
echo -e "OpenFGA: ${OPENFGA_URL}"
echo -e "RSFGA:   ${RSFGA_URL}"
echo

# Check both servers
echo -e "${BLUE}Checking servers...${NC}"
openfga_status="offline"
rsfga_status="offline"

if curl -sf "${OPENFGA_URL}/healthz" > /dev/null 2>&1 || curl -sf "${OPENFGA_URL}/health" > /dev/null 2>&1; then
    openfga_status="online"
    echo -e "  OpenFGA: ${GREEN}online${NC}"
else
    echo -e "  OpenFGA: ${RED}offline${NC}"
fi

if curl -sf "${RSFGA_URL}/health" > /dev/null 2>&1; then
    rsfga_status="online"
    echo -e "  RSFGA:   ${GREEN}online${NC}"
else
    echo -e "  RSFGA:   ${RED}offline${NC}"
fi

if [ "$openfga_status" = "offline" ] && [ "$rsfga_status" = "offline" ]; then
    echo -e "${RED}Both servers are offline!${NC}"
    exit 1
fi

echo

# Results tracking
declare -A OPENFGA_RESULTS
declare -A RSFGA_RESULTS

run_test() {
    local test_file="$1"
    local api_url="$2"
    local test_name=$(basename "$test_file" .fga.yaml)

    # Create store
    store_result=$(fga store create --api-url "${api_url}" --name "compare-${test_name}-$$" 2>&1) || return 1
    store_id=$(echo "$store_result" | grep -oE '"id":"[^"]+"' | head -1 | cut -d'"' -f4) || return 1
    [ -z "$store_id" ] && return 1

    # Run test
    local exit_code=0
    fga model test \
        --api-url "${api_url}" \
        --store-id "${store_id}" \
        --tests "${test_file}" > /dev/null 2>&1 || exit_code=$?

    # Cleanup
    fga store delete --api-url "${api_url}" --store-id "${store_id}" > /dev/null 2>&1 || true

    return $exit_code
}

# Run tests
echo -e "${BLUE}Running tests...${NC}"
echo
printf "%-40s %-12s %-12s\n" "Test" "OpenFGA" "RSFGA"
printf "%-40s %-12s %-12s\n" "----" "-------" "-----"

for test_file in "${TESTS_DIR}"/*.fga.yaml; do
    test_name=$(basename "$test_file" .fga.yaml)

    openfga_result="-"
    rsfga_result="-"

    # Test OpenFGA
    if [ "$openfga_status" = "online" ]; then
        if run_test "$test_file" "$OPENFGA_URL"; then
            openfga_result="PASS"
            OPENFGA_RESULTS["$test_name"]="pass"
        else
            openfga_result="FAIL"
            OPENFGA_RESULTS["$test_name"]="fail"
        fi
    else
        OPENFGA_RESULTS["$test_name"]="skip"
    fi

    # Test RSFGA
    if [ "$rsfga_status" = "online" ]; then
        if run_test "$test_file" "$RSFGA_URL"; then
            rsfga_result="PASS"
            RSFGA_RESULTS["$test_name"]="pass"
        else
            rsfga_result="FAIL"
            RSFGA_RESULTS["$test_name"]="fail"
        fi
    else
        RSFGA_RESULTS["$test_name"]="skip"
    fi

    # Color the results
    if [ "$openfga_result" = "PASS" ]; then
        openfga_colored="${GREEN}PASS${NC}"
    elif [ "$openfga_result" = "FAIL" ]; then
        openfga_colored="${RED}FAIL${NC}"
    else
        openfga_colored="${YELLOW}-${NC}"
    fi

    if [ "$rsfga_result" = "PASS" ]; then
        rsfga_colored="${GREEN}PASS${NC}"
    elif [ "$rsfga_result" = "FAIL" ]; then
        rsfga_colored="${RED}FAIL${NC}"
    else
        rsfga_colored="${YELLOW}-${NC}"
    fi

    # Check compatibility
    compat=""
    if [ "$openfga_result" = "$rsfga_result" ] && [ "$openfga_result" != "-" ]; then
        compat="${GREEN}✓${NC}"
    elif [ "$openfga_result" != "-" ] && [ "$rsfga_result" != "-" ]; then
        compat="${RED}✗${NC}"
    fi

    printf "%-40s " "$test_name"
    echo -e "${openfga_colored}         ${rsfga_colored}        ${compat}"
done

echo
echo -e "${CYAN}============================================${NC}"
echo -e "${CYAN}  Summary${NC}"
echo -e "${CYAN}============================================${NC}"

# Count results
openfga_pass=0
openfga_fail=0
rsfga_pass=0
rsfga_fail=0
compatible=0
incompatible=0

for test_name in "${!OPENFGA_RESULTS[@]}"; do
    case "${OPENFGA_RESULTS[$test_name]}" in
        pass) ((openfga_pass++)) ;;
        fail) ((openfga_fail++)) ;;
    esac
done

for test_name in "${!RSFGA_RESULTS[@]}"; do
    case "${RSFGA_RESULTS[$test_name]}" in
        pass) ((rsfga_pass++)) ;;
        fail) ((rsfga_fail++)) ;;
    esac
done

for test_name in "${!OPENFGA_RESULTS[@]}"; do
    if [ "${OPENFGA_RESULTS[$test_name]}" = "${RSFGA_RESULTS[$test_name]}" ]; then
        ((compatible++))
    elif [ "${OPENFGA_RESULTS[$test_name]}" != "skip" ] && [ "${RSFGA_RESULTS[$test_name]}" != "skip" ]; then
        ((incompatible++))
    fi
done

echo
echo "OpenFGA Results:"
echo -e "  Passed: ${GREEN}${openfga_pass}${NC}"
echo -e "  Failed: ${RED}${openfga_fail}${NC}"
echo
echo "RSFGA Results:"
echo -e "  Passed: ${GREEN}${rsfga_pass}${NC}"
echo -e "  Failed: ${RED}${rsfga_fail}${NC}"
echo
echo "Compatibility:"
echo -e "  Matching:     ${GREEN}${compatible}${NC}"
echo -e "  Incompatible: ${RED}${incompatible}${NC}"

if [ $incompatible -gt 0 ]; then
    echo
    echo -e "${RED}Warning: Some tests have different results between OpenFGA and RSFGA${NC}"
    exit 1
else
    echo
    echo -e "${GREEN}All tests have matching results!${NC}"
    exit 0
fi
