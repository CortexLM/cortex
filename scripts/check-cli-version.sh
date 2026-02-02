#!/bin/bash
#
# Check CLI Version Consistency
# 
# This script verifies that the version in VERSION_CLI matches:
# 1. The workspace version in Cargo.toml
# 2. The git tag (if provided via CLI_RELEASE_VERSION env var)
#
# Usage:
#   ./scripts/check-cli-version.sh            # Basic check
#   CLI_RELEASE_VERSION=0.0.4 ./scripts/check-cli-version.sh  # Also verify release tag
#

set -e

# Get the repository root
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "=== Cortex CLI Version Check ==="
echo ""

# Read VERSION_CLI file
VERSION_FILE="$REPO_ROOT/VERSION_CLI"
if [ ! -f "$VERSION_FILE" ]; then
    echo -e "${RED}ERROR: VERSION_CLI file not found at $VERSION_FILE${NC}"
    exit 1
fi

VERSION_CLI=$(cat "$VERSION_FILE" | tr -d '[:space:]')
echo "VERSION_CLI file:      $VERSION_CLI"

# Extract version from workspace Cargo.toml
CARGO_TOML="$REPO_ROOT/Cargo.toml"
if [ ! -f "$CARGO_TOML" ]; then
    echo -e "${RED}ERROR: Cargo.toml not found at $CARGO_TOML${NC}"
    exit 1
fi

# Parse the workspace.package.version from Cargo.toml
# We need to find the line after [workspace.package] that starts with version
WORKSPACE_VERSION=$(grep -A20 '^\[workspace\.package\]' "$CARGO_TOML" | grep '^version' | head -1 | sed 's/.*=\s*"\([^"]*\)".*/\1/')
echo "Workspace Cargo.toml:  $WORKSPACE_VERSION"

# Also check cortex-cli/Cargo.toml uses workspace version
CORTEX_CLI_CARGO="$REPO_ROOT/cortex-cli/Cargo.toml"
if [ -f "$CORTEX_CLI_CARGO" ]; then
    CLI_VERSION_LINE=$(grep '^version' "$CORTEX_CLI_CARGO" | head -1)
    if [[ "$CLI_VERSION_LINE" == *"workspace = true"* ]]; then
        echo "cortex-cli/Cargo.toml: uses workspace version ✓"
        CLI_USES_WORKSPACE=true
    else
        # Extract the version if it's hardcoded
        CLI_VERSION=$(echo "$CLI_VERSION_LINE" | sed 's/.*=\s*"\([^"]*\)".*/\1/')
        echo "cortex-cli/Cargo.toml: $CLI_VERSION (WARNING: should use workspace)"
        CLI_USES_WORKSPACE=false
    fi
fi

echo ""

# Check consistency
ERRORS=0

if [ "$VERSION_CLI" != "$WORKSPACE_VERSION" ]; then
    echo -e "${RED}ERROR: VERSION_CLI ($VERSION_CLI) does not match workspace version ($WORKSPACE_VERSION)${NC}"
    ERRORS=$((ERRORS + 1))
else
    echo -e "${GREEN}✓ VERSION_CLI matches workspace version${NC}"
fi

if [ "$CLI_USES_WORKSPACE" = false ]; then
    echo -e "${YELLOW}WARNING: cortex-cli/Cargo.toml should use 'version.workspace = true'${NC}"
fi

# Check against release version if provided (used in CI)
if [ -n "$CLI_RELEASE_VERSION" ]; then
    echo ""
    echo "Release version:       $CLI_RELEASE_VERSION"
    
    if [ "$VERSION_CLI" != "$CLI_RELEASE_VERSION" ]; then
        echo -e "${RED}ERROR: VERSION_CLI ($VERSION_CLI) does not match release version ($CLI_RELEASE_VERSION)${NC}"
        ERRORS=$((ERRORS + 1))
    else
        echo -e "${GREEN}✓ VERSION_CLI matches release version${NC}"
    fi
fi

echo ""

if [ $ERRORS -gt 0 ]; then
    echo -e "${RED}Version check failed with $ERRORS error(s)${NC}"
    echo ""
    echo "To fix version inconsistencies:"
    echo "1. Update VERSION_CLI with the correct version"
    echo "2. Update [workspace.package] version in Cargo.toml to match"
    echo "3. Ensure cortex-cli/Cargo.toml uses 'version.workspace = true'"
    exit 1
fi

echo -e "${GREEN}All version checks passed!${NC}"
exit 0
