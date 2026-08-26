#!/bin/bash
# Fetch the two inputs crates.io excludes, at the revisions `v8/DEPS` pins.
#
#   bash pane-deps.sh
#
# **Why anything is missing.** A crates.io package is capped at 10 MB packed, so
# this crate's `exclude` list drops `third_party/icu/common/icudtl.dat` and all of
# `third_party/rust/chromium_crates_io` -- 302 vendored Rust crates. Neither is
# optional for a source build: the ICU data file is what ICU reads at runtime, and
# the ICU4X crates among the vendored ones are what V8 builds `Temporal` out of.
# `typeof Temporal` answers `object` in the browser this fork serves, so building
# without them would be a different browser.
#
# **Why the revisions are not written here.** They are already written down, in
# `v8/DEPS`, and this reads them through the crate's own parser (`tools/v8_deps.py`).
# Hardcoding them would mean two places to update at the next V8 roll, and one of
# them would eventually be wrong.
#
# **Why a subtree archive and not a git fetch.** `git fetch` of a revision would
# verify its own SHA-1, which this cannot -- gitiles builds each `+archive`
# tarball on request, so its bytes are not stable and there is no checksum to pin.
# What a fetch would also do is lay the *entire* upstream tree over this one:
# `third_party/icu` upstream is a full ICU checkout, while the crate ships a
# curated subset of it, and the difference would silently change what gets built.
# An `+archive` of one path cannot write outside that path. The compensating check
# is at the bottom of this script, and it is the one that matters in practice:
# every vendored crate the build actually compiles must be the version the crate's
# own `BUILD.gn` shell says it is.
set -euo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

die() { echo "pane-deps: $*" >&2; exit 1; }

[ -f v8/DEPS ] || die "no v8/DEPS beside this script -- run it from the crate it lives in"

# --- what to fetch, out of v8/DEPS ----------------------------------------
dep() {
  python3 -c '
import sys
sys.path.insert(0, "tools")
from v8_deps import deps
print(deps[sys.argv[1]])
' "$1"
}

split_dep() {
  # `<url>@<rev>` -> two validated fields. Validated because they are about to be
  # interpolated into a URL: a malformed DEPS should stop here rather than send a
  # request somewhere unintended.
  local spec=$1 url rev
  url=${spec%@*}
  rev=${spec##*@}
  url=${url%.git}
  [[ $url =~ ^https://chromium\.googlesource\.com/[A-Za-z0-9._/-]+$ ]] \
    || die "refusing a dependency URL that is not chromium.googlesource.com: $url"
  [[ $rev =~ ^[0-9a-f]{40}$ ]] || die "not a git revision: $rev"
  echo "$url" "$rev"
}

read -r ICU_URL ICU_REV < <(split_dep "$(dep third_party/icu)")
read -r RUST_URL RUST_REV < <(split_dep "$(dep third_party/rust)")

echo "icu:   $ICU_URL @ $ICU_REV"
echo "rust:  $RUST_URL @ $RUST_REV"

# --- fetch ----------------------------------------------------------------
# Idempotent: an existing tree that passes the check at the bottom is left alone,
# because re-extracting 302 crates on every build would be minutes for nothing.
fetch_subtree() {
  local url=$1 rev=$2 path=$3 dest=$4 tmp
  tmp=$(mktemp -d)
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN
  echo "fetching $path.tar.gz"
  curl -fSL --retry 3 --no-progress-meter -o "$tmp/a.tar.gz" "$url/+archive/$rev/$path.tar.gz"
  mkdir -p "$dest"
  tar xzf "$tmp/a.tar.gz" -C "$dest"
}

if [ -f third_party/icu/common/icudtl.dat ] && [ -f third_party/icu/common/icudtb.dat ]; then
  echo "icu:   already present"
else
  fetch_subtree "$ICU_URL" "$ICU_REV" common third_party/icu/common
fi

if [ -d third_party/rust/chromium_crates_io/vendor ]; then
  echo "rust:  already present"
else
  fetch_subtree "$RUST_URL" "$RUST_REV" chromium_crates_io third_party/rust/chromium_crates_io
fi

# --- verify ---------------------------------------------------------------
# Every `third_party/rust/<crate>/v*/BUILD.gn` in this tree names the exact
# version it expects to find in the vendored copy. A single disagreement means the
# two were rolled at different times, and the build would either fail late or
# quietly compile a different `Temporal`.
checked=0
while IFS= read -r gn; do
  want=$(sed -n 's/.*cargo_pkg_version = "\(.*\)".*/\1/p' "$gn" | head -1)
  root=$(sed -n 's|.*crate_root = "//third_party/rust/chromium_crates_io/vendor/\([^/]*\)/.*|\1|p' "$gn" | head -1)
  [ -n "$want" ] && [ -n "$root" ] || continue
  toml="third_party/rust/chromium_crates_io/vendor/$root/Cargo.toml"
  [ -f "$toml" ] || die "$root is missing from the vendored crates, and $gn wants $want"
  got=$(sed -n 's/^version = "\(.*\)".*/\1/p' "$toml" | head -1)
  [ "$want" = "$got" ] || die "$root: this tree expects $want, $RUST_REV has $got"
  checked=$((checked + 1))
done < <(find third_party/rust -mindepth 3 -maxdepth 3 -name BUILD.gn)
[ "$checked" -gt 0 ] || die "found no BUILD.gn shells to check -- is third_party/rust intact?"
echo "check: $checked vendored crates agree on their versions"

cat <<'EOF'

Ready. To build the archive:

    RUSTY_V8_ARCHIVE= V8_FROM_SOURCE=1 cargo build --release

run from the *embedder's* workspace, not from here. The empty RUSTY_V8_ARCHIVE is
required whenever that workspace sets it: leaving it set copies the stale archive
back over what this build produces.
EOF
