#!/usr/bin/env bash
# Downloads CI-built artifacts into the web root so a clone can run the app
# (and, once guest profiles are built, boot Linux) without local toolchains.
# Requires the GitHub CLI authenticated for this repository.
#
# Usage: fetch-ci-assets.sh [run-id]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"
RUN_ID="${1:-$(gh run list --workflow=CI --status=success --limit 1 --json databaseId --jq '.[0].databaseId')}"
test -n "${RUN_ID}" || { echo "no successful CI run found" >&2; exit 1; }

mkdir -p web/public/wasm
gh run download "${RUN_ID}" -n wasmnux-wasm-pkg -D web/public/wasm
echo "WASM core installed to web/public/wasm (from run ${RUN_ID})"

GUEST_RUN="$(gh run list --workflow='Guest images' --status=success --limit 1 --json databaseId --jq '.[0].databaseId // empty')"
if [ -n "${GUEST_RUN}" ]; then
  mkdir -p web/public/guest
  gh run download "${GUEST_RUN}" -n guest-profiles -D web/public/guest || \
    echo "NOTE: guest-profiles artifact not present on run ${GUEST_RUN} (only the fast pipeline may have run)."
else
  echo "NOTE: no successful guest build yet; profiles will show 'guest assets not built'."
fi
