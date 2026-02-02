#!/bin/bash
#
# Bump CLI Version - Automated Version Management
#
# This script automatically bumps the version in both:
# 1. VERSION_CLI file
# 2. workspace.package.version in Cargo.toml
#
# Usage:
#   ./scripts/bump-version.sh patch    # 0.0.6 -> 0.0.7
#   ./scripts/bump-version.sh minor    # 0.0.6 -> 0.1.0
#   ./scripts/bump-version.sh major    # 0.0.6 -> 1.0.0
#   ./scripts/bump-version.sh 1.2.3    # Set exact version
#   ./scripts/bump-version.sh 1.0.0-beta.1  # Prerelease version
#
# Options:
#   --dry-run    Show what would be changed without making changes
#   --help       Show this help message
#

set -euo pipefail

# Get the repository root
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Files to update
VERSION_FILE="$REPO_ROOT/VERSION_CLI"
CARGO_TOML="$REPO_ROOT/Cargo.toml"

DRY_RUN=false
BUMP_TYPE=""

# Parse options
while [[ $# -gt 0 ]]; do
    case $1 in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [options] <patch|minor|major|X.Y.Z>"
            echo ""
            echo "Bump the CLI version automatically."
            echo ""
            echo "Arguments:"
            echo "  patch     Increment patch version (0.0.6 -> 0.0.7)"
            echo "  minor     Increment minor version (0.0.6 -> 0.1.0)"
            echo "  major     Increment major version (0.0.6 -> 1.0.0)"
            echo "  X.Y.Z     Set exact version (e.g., 1.2.3 or 1.0.0-beta.1)"
            echo ""
            echo "Prerelease versions:"
            echo "  Supports semver prerelease format: X.Y.Z-<prerelease>"
            echo "  Examples: 1.0.0-alpha, 1.0.0-beta.1, 2.0.0-rc.2"
            echo ""
            echo "Options:"
            echo "  --dry-run Show what would be changed without making changes"
            echo "  --help    Show this help message"
            echo ""
            echo "Examples:"
            echo "  $0 patch              # Bump patch version"
            echo "  $0 minor              # Bump minor version"
            echo "  $0 major              # Bump major version"
            echo "  $0 2.0.0              # Set version to 2.0.0"
            echo "  $0 1.0.0-beta.1       # Set prerelease version"
            echo "  $0 --dry-run patch    # Preview patch bump"
            exit 0
            ;;
        -*)
            echo -e "${RED}ERROR: Unknown option: $1${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
        *)
            if [ -n "$BUMP_TYPE" ]; then
                echo -e "${RED}ERROR: Multiple version arguments provided: '$BUMP_TYPE' and '$1'${NC}"
                echo "Only one version argument (patch, minor, major, or X.Y.Z) is allowed"
                exit 1
            fi
            BUMP_TYPE="$1"
            shift
            ;;
    esac
done

if [ -z "$BUMP_TYPE" ]; then
    echo -e "${RED}ERROR: Please specify bump type (patch, minor, major) or exact version (X.Y.Z)${NC}"
    echo "Use --help for usage information"
    exit 1
fi

# Read current version
if [ ! -f "$VERSION_FILE" ]; then
    echo -e "${RED}ERROR: VERSION_CLI file not found at $VERSION_FILE${NC}"
    exit 1
fi

CURRENT_VERSION=$(cat "$VERSION_FILE" | tr -d '[:space:]')
echo -e "${BLUE}Current version:${NC} $CURRENT_VERSION"

# Strip prerelease suffix for parsing (e.g., 1.0.0-beta.1 -> 1.0.0)
BASE_VERSION="${CURRENT_VERSION%%-*}"

# Parse version components
IFS='.' read -r MAJOR MINOR PATCH <<< "$BASE_VERSION"

# Validate current version format
if ! [[ "$MAJOR" =~ ^[0-9]+$ ]] || ! [[ "$MINOR" =~ ^[0-9]+$ ]] || ! [[ "$PATCH" =~ ^[0-9]+$ ]]; then
    echo -e "${RED}ERROR: Invalid current version format: $CURRENT_VERSION${NC}"
    echo "Expected base format: X.Y.Z (e.g., 0.0.6 or 1.0.0-beta.1)"
    exit 1
fi

# Version comparison function
# Returns: 0 if equal, 1 if v1 > v2, 2 if v1 < v2
compare_versions() {
    local v1_base="${1%%-*}"
    local v2_base="${2%%-*}"
    
    local v1_major v1_minor v1_patch
    local v2_major v2_minor v2_patch
    
    IFS='.' read -r v1_major v1_minor v1_patch <<< "$v1_base"
    IFS='.' read -r v2_major v2_minor v2_patch <<< "$v2_base"
    
    if [ "$v1_major" -gt "$v2_major" ]; then return 1; fi
    if [ "$v1_major" -lt "$v2_major" ]; then return 2; fi
    if [ "$v1_minor" -gt "$v2_minor" ]; then return 1; fi
    if [ "$v1_minor" -lt "$v2_minor" ]; then return 2; fi
    if [ "$v1_patch" -gt "$v2_patch" ]; then return 1; fi
    if [ "$v1_patch" -lt "$v2_patch" ]; then return 2; fi
    return 0
}

# Calculate new version
case $BUMP_TYPE in
    patch)
        NEW_VERSION="$MAJOR.$MINOR.$((PATCH + 1))"
        ;;
    minor)
        NEW_VERSION="$MAJOR.$((MINOR + 1)).0"
        ;;
    major)
        NEW_VERSION="$((MAJOR + 1)).0.0"
        ;;
    *)
        # Assume it's an exact version - validate format
        if [[ "$BUMP_TYPE" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$ ]]; then
            NEW_VERSION="$BUMP_TYPE"
            # Check for potential version downgrade
            cmp_result=0
            compare_versions "$CURRENT_VERSION" "$NEW_VERSION" || cmp_result=$?
            if [ $cmp_result -eq 1 ]; then
                echo -e "${YELLOW}⚠ WARNING: You are DOWNGRADING from $CURRENT_VERSION to $NEW_VERSION${NC}"
            fi
        else
            echo -e "${RED}ERROR: Invalid version format: $BUMP_TYPE${NC}"
            echo "Expected: patch, minor, major, or X.Y.Z (e.g., 1.2.3 or 1.0.0-beta.1)"
            exit 1
        fi
        ;;
esac

echo -e "${BLUE}New version:${NC}     $NEW_VERSION"
echo ""

if [ "$DRY_RUN" = true ]; then
    echo -e "${YELLOW}=== DRY RUN MODE - No changes will be made ===${NC}"
    echo ""
fi

# Update VERSION_CLI
echo -e "${BLUE}Updating VERSION_CLI...${NC}"
if [ "$DRY_RUN" = true ]; then
    echo "  Would write '$NEW_VERSION' to $VERSION_FILE"
else
    # Use atomic file operations: write to temp file first, then move
    VERSION_FILE_TMP=$(mktemp "${VERSION_FILE}.tmp.XXXXXX")
    echo "$NEW_VERSION" > "$VERSION_FILE_TMP"
    
    # Verify the temp file was written correctly
    if [ ! -s "$VERSION_FILE_TMP" ] || [ "$(cat "$VERSION_FILE_TMP")" != "$NEW_VERSION" ]; then
        rm -f "$VERSION_FILE_TMP" 2>/dev/null || true
        echo -e "${RED}ERROR: Failed to write VERSION_CLI temp file${NC}"
        exit 1
    fi
    
    # Atomically replace the original file
    mv "$VERSION_FILE_TMP" "$VERSION_FILE"
    echo -e "  ${GREEN}✓ Updated VERSION_CLI${NC}"
fi

# Update Cargo.toml workspace version
echo -e "${BLUE}Updating Cargo.toml workspace version...${NC}"

# Use awk to update the version in [workspace.package] section
if [ "$DRY_RUN" = true ]; then
    echo "  Would update version to \"$NEW_VERSION\" in $CARGO_TOML"
else
    # Use atomic file operations: write to temp file first, then move
    CARGO_TOML_TMP=$(mktemp "${CARGO_TOML}.tmp.XXXXXX")
    
    # Cleanup function for trap
    cleanup() {
        rm -f "$CARGO_TOML_TMP" 2>/dev/null || true
    }
    trap cleanup EXIT
    
    # Use awk to update the version only in [workspace.package] section
    awk -v new_ver="$NEW_VERSION" '
        /^\[workspace\.package\]/ { in_section = 1 }
        /^\[/ && !/^\[workspace\.package\]/ { in_section = 0 }
        in_section && /^version[[:space:]]*=/ {
            sub(/"[^"]*"/, "\"" new_ver "\"")
        }
        { print }
    ' "$CARGO_TOML" > "$CARGO_TOML_TMP"
    
    # Verify the temp file is not empty and has expected content
    if [ ! -s "$CARGO_TOML_TMP" ]; then
        echo -e "${RED}ERROR: Failed to update Cargo.toml - awk produced empty output${NC}"
        exit 1
    fi
    
    # Verify the new version is in the temp file
    if ! grep -q "version = \"$NEW_VERSION\"" "$CARGO_TOML_TMP"; then
        echo -e "${RED}ERROR: Failed to update version in Cargo.toml${NC}"
        exit 1
    fi
    
    # Atomically replace the original file
    mv "$CARGO_TOML_TMP" "$CARGO_TOML"
    
    # Clear trap since file was successfully moved
    trap - EXIT
    
    echo -e "  ${GREEN}✓ Updated Cargo.toml${NC}"
fi

echo ""

# Verify consistency
echo -e "${BLUE}Verifying version consistency...${NC}"
if [ "$DRY_RUN" = true ]; then
    echo "  Would run: ./scripts/check-cli-version.sh"
else
    if "$REPO_ROOT/scripts/check-cli-version.sh"; then
        echo ""
        echo -e "${GREEN}=== Version bump complete! ===${NC}"
        echo ""
        echo "Next steps:"
        echo "  1. Review the changes: git diff"
        echo "  2. Commit: git commit -am \"chore: bump version to $NEW_VERSION\""
        echo "  3. Tag for release: git tag v$NEW_VERSION"
        echo "  4. Push: git push && git push --tags"
    else
        echo ""
        echo -e "${RED}ERROR: Version consistency check failed!${NC}"
        exit 1
    fi
fi
