#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
npx tsx packages/coding-agent/src/core/kernel/bootstrap-cli.ts
