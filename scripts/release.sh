#!/bin/bash
#
# Release Script - Automated Release Management
#
# This script automates the release process:
# 1. Bumps version (patch, minor, or major)
# 2. Creates git commit with version bump
# 3. Creates git tag
# 4. Pushes to remote (optional)
#
# Usage:
#   ./scripts/release.sh patch           # Patch release (0.0.6 -> 0.0.7)
#   ./scripts/release.sh minor           # Minor release (0.0.6 -> 0.1.0)
#   ./scripts/release.sh major           # Major release (0.0.6 -> 1.0.0)
#   ./scripts/release.sh 1.2.3           # Specific version release
#   ./scripts/release.sh patch --push    # Release and push to remote
#
# Options:
#   --push       Push commits and tags to remote
#   --dry-run    Show what would be done without making changes
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

PUSH_TO_REMOTE=false
DRY_RUN=false
VERSION_ARG=""

# Parse options
while [[ $# -gt 0 ]]; do
    case $1 in
        --push)
            PUSH_TO_REMOTE=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [options] <patch|minor|major|X.Y.Z>"
            echo ""
            echo "Automate the release process with version bumping and tagging."
            echo ""
            echo "Arguments:"
            echo "  patch     Create patch release (0.0.6 -> 0.0.7)"
            echo "  minor     Create minor release (0.0.6 -> 0.1.0)"
            echo "  major     Create major release (0.0.6 -> 1.0.0)"
            echo "  X.Y.Z     Create release with specific version"
            echo ""
            echo "Options:"
            echo "  --push    Push commits and tags to remote after release"
            echo "  --dry-run Show what would be done without making changes"
            echo "  --help    Show this help message"
            echo ""
            echo "Examples:"
            echo "  $0 patch              # Create patch release locally"
            echo "  $0 minor --push       # Create minor release and push"
            echo "  $0 1.0.0 --push       # Release version 1.0.0 and push"
            echo "  $0 --dry-run patch    # Preview patch release"
            exit 0
            ;;
        -*)
            echo -e "${RED}ERROR: Unknown option: $1${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
        *)
            if [ -n "$VERSION_ARG" ]; then
                echo -e "${RED}ERROR: Multiple version arguments provided${NC}"
                exit 1
            fi
            VERSION_ARG="$1"
            shift
            ;;
    esac
done

if [ -z "$VERSION_ARG" ]; then
    echo -e "${RED}ERROR: Please specify version bump type (patch, minor, major) or exact version${NC}"
    echo "Use --help for usage information"
    exit 1
fi

# Change to repository root
cd "$REPO_ROOT"

# Check for clean working directory
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo -e "${RED}ERROR: Working directory is not clean${NC}"
    echo "Please commit or stash your changes before creating a release"
    git status --short
    exit 1
fi

# Get current version
CURRENT_VERSION=$(cat VERSION_CLI | tr -d '[:space:]')
echo -e "${BLUE}Current version:${NC} $CURRENT_VERSION"

if [ "$DRY_RUN" = true ]; then
    echo -e "${YELLOW}=== DRY RUN MODE ===${NC}"
    echo ""
fi

# Step 1: Bump version
echo -e "${BLUE}Step 1: Bumping version...${NC}"
if [ "$DRY_RUN" = true ]; then
    ./scripts/bump-version.sh --dry-run "$VERSION_ARG"
else
    ./scripts/bump-version.sh "$VERSION_ARG"
fi

# Get new version
NEW_VERSION=$(cat VERSION_CLI | tr -d '[:space:]')

# Step 2: Update Cargo.lock
echo ""
echo -e "${BLUE}Step 2: Updating Cargo.lock...${NC}"
if [ "$DRY_RUN" = true ]; then
    echo "  Would run: cargo check (to update Cargo.lock)"
else
    cargo check --workspace 2>/dev/null || true
fi

# Step 3: Commit changes
echo ""
echo -e "${BLUE}Step 3: Creating version commit...${NC}"
COMMIT_MSG="chore: bump version to $NEW_VERSION"
if [ "$DRY_RUN" = true ]; then
    echo "  Would commit: $COMMIT_MSG"
    echo "  Files: VERSION_CLI, Cargo.toml, Cargo.lock"
else
    git add VERSION_CLI Cargo.toml Cargo.lock
    git commit -m "$COMMIT_MSG"
    echo -e "  ${GREEN}✓ Created commit${NC}"
fi

# Step 4: Create tag
echo ""
echo -e "${BLUE}Step 4: Creating version tag...${NC}"
TAG_NAME="v$NEW_VERSION"
if [ "$DRY_RUN" = true ]; then
    echo "  Would create tag: $TAG_NAME"
else
    git tag "$TAG_NAME"
    echo -e "  ${GREEN}✓ Created tag: $TAG_NAME${NC}"
fi

# Step 5: Push (if requested)
if [ "$PUSH_TO_REMOTE" = true ]; then
    echo ""
    echo -e "${BLUE}Step 5: Pushing to remote...${NC}"
    if [ "$DRY_RUN" = true ]; then
        echo "  Would push: commits and tag $TAG_NAME"
    else
        git push origin HEAD
        git push origin "$TAG_NAME"
        echo -e "  ${GREEN}✓ Pushed to remote${NC}"
    fi
fi

# Summary
echo ""
echo -e "${GREEN}=== Release Complete ===${NC}"
echo ""
echo "  Version: $CURRENT_VERSION -> $NEW_VERSION"
echo "  Tag:     $TAG_NAME"
echo ""

if [ "$PUSH_TO_REMOTE" = false ] && [ "$DRY_RUN" = false ]; then
    echo "Next steps:"
    echo "  git push origin HEAD         # Push commit"
    echo "  git push origin $TAG_NAME    # Push tag (triggers CI release)"
fi
