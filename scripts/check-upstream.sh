#!/usr/bin/env bash
# Fails when a dependency Voice Strip pins by hand has a newer upstream:
# tract (through the DeepFilterNet fork, which Dependabot does not follow)
# and DeepFilterNet itself. Run weekly by .github/workflows/upstream.yml.
#
# After reviewing an update, bump the pin (see docs/denoise.md, "Maintaining
# the fork") or the reviewed values below, so the check passes again.

set -euo pipefail

# Upstream DeepFilterNet state last reviewed: latest release and main commit.
REVIEWED_DFN_RELEASE=v0.5.6
REVIEWED_DFN_MAIN=d375b2d8309e0935d165700c91da9de862a99c31

cd "$(dirname "$0")/.."
api() { curl -fsSL -A "voice-strip-upstream-check" "$@"; }
gh_api() {
  if [[ -n ${GITHUB_TOKEN:-} ]]; then
    api -H "Authorization: Bearer $GITHUB_TOKEN" "https://api.github.com/$1"
  else
    api "https://api.github.com/$1"
  fi
}

stale=0

pinned=$(awk '/^name = "tract-core"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)
latest=$(api https://crates.io/api/v1/crates/tract-core | jq -r .crate.max_stable_version)
if [[ $pinned == "$latest" ]]; then
  echo "tract-core: $pinned is the latest"
else
  echo "::warning::tract-core $latest is out (Voice Strip uses $pinned): update the DeepFilterNet fork"
  stale=1
fi

release=$(gh_api repos/Rikorose/DeepFilterNet/releases/latest | jq -r .tag_name)
if [[ $release == "$REVIEWED_DFN_RELEASE" ]]; then
  echo "DeepFilterNet release: $release, reviewed"
else
  echo "::warning::DeepFilterNet released $release (reviewed: $REVIEWED_DFN_RELEASE)"
  stale=1
fi

main=$(gh_api repos/Rikorose/DeepFilterNet/commits/main | jq -r .sha)
if [[ $main == "$REVIEWED_DFN_MAIN" ]]; then
  echo "DeepFilterNet main: ${main:0:7}, reviewed"
else
  echo "::warning::DeepFilterNet main moved to ${main:0:7} (reviewed: ${REVIEWED_DFN_MAIN:0:7}): https://github.com/Rikorose/DeepFilterNet/compare/$REVIEWED_DFN_MAIN...$main"
  stale=1
fi

exit "$stale"
