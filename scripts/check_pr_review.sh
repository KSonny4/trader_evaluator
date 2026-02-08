#!/usr/bin/env bash
# check_pr_review.sh — Validates that a PR body contains a structured Code Review section.
#
# Usage:
#   echo "$PR_BODY" | ./scripts/check_pr_review.sh
#   ./scripts/check_pr_review.sh < pr_body.txt
#
# Exit codes:
#   0 — Code Review section found with all required sub-sections
#   1 — Missing or incomplete Code Review section
#
# Required format in PR body:
#   ## Code Review
#   ### Strengths
#   (at least one non-empty line)
#   ### Issues
#   (at least one non-empty line)
#   ### Assessment
#   (at least one non-empty line)

set -euo pipefail

PR_BODY=$(cat)

if [ -z "$PR_BODY" ]; then
    echo "FAIL: PR body is empty"
    echo ""
    echo "Your PR must include a '## Code Review' section with:"
    echo "  ### Strengths"
    echo "  ### Issues"
    echo "  ### Assessment"
    echo ""
    echo "Run the requesting-code-review skill before creating your PR."
    exit 1
fi

# Use a single awk pass to detect all sections and their content.
# Output: one line per check result (e.g., "code_review=1", "strengths=1", etc.)
result=$(echo "$PR_BODY" | awk '
BEGIN {
    section = ""
    has_code_review = 0
    has_strengths = 0; strengths_content = 0
    has_issues = 0; issues_content = 0
    has_assessment = 0; assessment_content = 0
}
{
    line = tolower($0)
}
# Detect ## Code Review or # Code Review
line ~ /^#+ +code +review/ {
    has_code_review = 1
    section = ""
    next
}
# Detect ### Strengths or ## Strengths
line ~ /^#+ +strengths/ {
    has_strengths = 1
    section = "strengths"
    next
}
# Detect ### Issues or ## Issues
line ~ /^#+ +issues/ {
    has_issues = 1
    section = "issues"
    next
}
# Detect ### Assessment or ## Assessment
line ~ /^#+ +assessment/ {
    has_assessment = 1
    section = "assessment"
    next
}
# Any other header resets current section
line ~ /^#+ / {
    section = ""
    next
}
# Skip HTML comments and bare list markers (template placeholders)
/^[[:space:]]*<!--.*-->/ { next }
/^[[:space:]]*-[[:space:]]*$/ { next }
# Non-empty line in a tracked section (real content only)
section != "" && /[^ \t]/ {
    if (section == "strengths") strengths_content = 1
    if (section == "issues") issues_content = 1
    if (section == "assessment") assessment_content = 1
}
END {
    print "has_code_review=" has_code_review
    print "has_strengths=" has_strengths
    print "strengths_content=" strengths_content
    print "has_issues=" has_issues
    print "issues_content=" issues_content
    print "has_assessment=" has_assessment
    print "assessment_content=" assessment_content
}
')

# Parse awk output into bash variables (no eval — defense-in-depth)
has_code_review=0; has_strengths=0; strengths_content=0
has_issues=0; issues_content=0; has_assessment=0; assessment_content=0
while IFS='=' read -r key value; do
    case "$key" in
        has_code_review)   has_code_review="$value" ;;
        has_strengths)     has_strengths="$value" ;;
        strengths_content) strengths_content="$value" ;;
        has_issues)        has_issues="$value" ;;
        issues_content)    issues_content="$value" ;;
        has_assessment)    has_assessment="$value" ;;
        assessment_content) assessment_content="$value" ;;
    esac
done <<< "$result"

errors=()

if [ "$has_code_review" -eq 0 ]; then
    errors+=("Missing '## Code Review' section header")
fi

if [ "$has_strengths" -eq 0 ]; then
    errors+=("Missing '### Strengths' sub-section")
elif [ "$strengths_content" -eq 0 ]; then
    errors+=("'### Strengths' section is empty — add at least one finding")
fi

if [ "$has_issues" -eq 0 ]; then
    errors+=("Missing '### Issues' sub-section")
elif [ "$issues_content" -eq 0 ]; then
    errors+=("'### Issues' section is empty — add findings or write 'No issues found'")
fi

if [ "$has_assessment" -eq 0 ]; then
    errors+=("Missing '### Assessment' sub-section")
elif [ "$assessment_content" -eq 0 ]; then
    errors+=("'### Assessment' section is empty — add overall assessment")
fi

if [ ${#errors[@]} -gt 0 ]; then
    echo "FAIL: PR body is missing required Code Review content"
    echo ""
    for err in "${errors[@]}"; do
        echo "  - $err"
    done
    echo ""
    echo "Your PR must include a '## Code Review' section with:"
    echo "  ### Strengths"
    echo "  ### Issues"
    echo "  ### Assessment"
    echo ""
    echo "Run the requesting-code-review skill before creating your PR."
    exit 1
fi

echo "OK: Code Review section found with Strengths, Issues, and Assessment"
exit 0
