#!/usr/bin/env bash
# test_check_pr_review.sh — Tests for check_pr_review.sh
#
# Usage: ./scripts/test_check_pr_review.sh

set -uo pipefail
# Note: NOT using set -e because ((var++)) returns 1 when var=0,
# and assert functions intentionally handle non-zero exits.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CHECK="$SCRIPT_DIR/check_pr_review.sh"
PASS=0
FAIL=0

assert_pass() {
    local name="$1"
    local input="$2"
    if echo "$input" | "$CHECK" > /dev/null 2>&1; then
        echo "  PASS: $name"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $name (expected pass, got fail)"
        echo "$input" | "$CHECK" 2>&1 | sed 's/^/    /' || true
        FAIL=$((FAIL + 1))
    fi
}

assert_fail() {
    local name="$1"
    local input="$2"
    if echo "$input" | "$CHECK" > /dev/null 2>&1; then
        echo "  FAIL: $name (expected fail, got pass)"
        FAIL=$((FAIL + 1))
    else
        echo "  PASS: $name"
        PASS=$((PASS + 1))
    fi
}

echo "=== Testing check_pr_review.sh ==="
echo ""

# --- Happy path ---
echo "--- Happy path ---"

assert_pass "Complete review section" "## Summary
Some changes here

## Code Review

### Strengths
- Clean architecture
- Good test coverage

### Issues
- No issues found

### Assessment
Ready to merge"

assert_pass "Minimal valid review" "## Code Review
### Strengths
Good
### Issues
None
### Assessment
OK"

assert_pass "Review with ### headers under ## Code Review" "## Code Review
### Strengths
- Solid work
### Issues
- Minor: variable naming
### Assessment
Approved with minor nits"

# --- Missing sections ---
echo ""
echo "--- Missing sections ---"

assert_fail "Empty body" ""

assert_fail "No Code Review section at all" "## Summary
Just a summary, no review"

assert_fail "Code Review header but no sub-sections" "## Code Review
Some text but no structured sub-sections"

assert_fail "Missing Strengths" "## Code Review
### Issues
- Something
### Assessment
OK"

assert_fail "Missing Issues" "## Code Review
### Strengths
- Something
### Assessment
OK"

assert_fail "Missing Assessment" "## Code Review
### Strengths
- Something
### Issues
- Something"

# --- Empty sub-sections ---
echo ""
echo "--- Empty sub-sections ---"

assert_fail "Empty Strengths section" "## Code Review
### Strengths

### Issues
- Something
### Assessment
OK"

assert_fail "Empty Issues section" "## Code Review
### Strengths
- Something
### Issues

### Assessment
OK"

assert_fail "Empty Assessment section" "## Code Review
### Strengths
- Something
### Issues
- Something
### Assessment
"

# --- Case insensitivity ---
echo ""
echo "--- Case variations ---"

assert_pass "Lowercase 'code review'" "## code review
### strengths
Good
### issues
None
### assessment
OK"

assert_pass "Mixed case" "## Code review
### STRENGTHS
Good
### Issues
None
### Assessment
OK"

# --- Edge cases ---
echo ""
echo "--- Edge cases ---"

assert_pass "Review section buried in large PR body" "## Summary
Lots of changes

## Changes
- Changed foo
- Changed bar

## Testing
Ran all tests

## Code Review

### Strengths
- Well structured

### Issues
- No issues found

### Assessment
Ship it

## Notes
Some extra notes"

assert_pass "Single hash Code Review (# Code Review)" "# Code Review
## Strengths
Good
## Issues
None
## Assessment
OK"

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
