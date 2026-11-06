#!/usr/bin/env bash
# Cut a wayfinder release: bump every version carrier, build-check the bump,
# commit "chore(release): X.Y.Z" and create the annotated tag vX.Y.Z.
#
#   scripts/release.sh <X.Y.Z> [--dry-run] [--skip-build]
#
# A tag cut without the bump ships a binary whose --version names the previous
# release, so the carriers are rewritten and build-checked together. The
# script never pushes; it prints the two push commands at the end.
#
# Environment:
#   WAYFINDER_RELEASE_REMOTE  remote to verify against (default: origin)
set -euo pipefail

log() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
# A failure after the carriers were rewritten but before the commit landed.
die_recover_precommit() {
  printf 'error: %s\n' "$*" >&2
  printf 'nothing was committed; the edited carriers can be reverted with:\n  git checkout HEAD -- Cargo.toml Cargo.lock python/src/wayfinder_cli/__init__.py\n' >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: scripts/release.sh <X.Y.Z> [--dry-run] [--skip-build]

Cut a release from a clean main:

  1. bump Cargo.toml, the wayfinder / wayfinder-core / wayfinder-bench packages
     in Cargo.lock, and the Python __version__ to X.Y.Z
  2. cargo run --locked + executed --version check (unless --skip-build)
  3. commit "chore(release): X.Y.Z" and create annotated tag vX.Y.Z
  4. print the two push commands (the script never pushes)

Options:
  --dry-run     show what would change, then exit 0 without modifying anything
  --skip-build  skip the cargo checks (the release workflow still enforces them)
  -h, --help    show this help

Environment:
  WAYFINDER_RELEASE_REMOTE  remote to verify against (default: origin)
EOF
}

VERSION=""
DRY_RUN=0
SKIP_BUILD=0
for arg in "$@"; do
  case "$arg" in
    --dry-run)    DRY_RUN=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    -h|--help)    usage; exit 0 ;;
    -*)           die "unknown option: $arg (see --help)" ;;
    *)
      [ -z "$VERSION" ] || die "expected exactly one version argument, got '$VERSION' and '$arg'"
      VERSION="${arg#v}"
      ;;
  esac
done
[ -n "$VERSION" ] || { usage >&2; die "missing required <X.Y.Z> version"; }
VERSION_RE='^[0-9]+\.[0-9]+\.[0-9]+$'
[[ "$VERSION" =~ $VERSION_RE ]] || die "version must look like X.Y.Z, got '$VERSION'"

# Work from the repository root no matter where the script was invoked.
ROOT="$(cd "$(dirname "$0")/.." && pwd)" || die "cannot resolve the repository root from $0"
cd "$ROOT"

command -v git >/dev/null 2>&1 || die "git is required"
command -v python3 >/dev/null 2>&1 || die "python3 is required"
if [ "$SKIP_BUILD" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
  command -v cargo >/dev/null 2>&1 || die "cargo is required (or pass --skip-build)"
fi

# Current version: the first `version = "…"` line of Cargo.toml and the Python
# __version__; the two carriers must already agree before anything is touched.
CARGO_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
PY_VERSION="$(sed -n 's/^__version__ = "\([^"]*\)"/\1/p' python/src/wayfinder_cli/__init__.py | head -n 1)"
[ -n "$CARGO_VERSION" ] || die "no version line found in Cargo.toml (expected: version = \"X.Y.Z\")"
[ -n "$PY_VERSION" ] || die "no __version__ line found in python/src/wayfinder_cli/__init__.py"
[ "$CARGO_VERSION" = "$PY_VERSION" ] || die "version carriers disagree: Cargo.toml says $CARGO_VERSION, Python says $PY_VERSION"
CURRENT="$CARGO_VERSION"
log "current version: $CURRENT -> target: $VERSION"

# A previous failed run may have left edits behind; recover before clobbering.
if [ "$CURRENT" = "$VERSION" ]; then
  die "the tree is already at $VERSION; a previous run may have left edits behind. Revert first:
  git checkout HEAD -- Cargo.toml Cargo.lock python/src/wayfinder_cli/__init__.py"
fi

# Releases only move forward (tuple comparison — no sort -V).
if ! python3 - "$CURRENT" "$VERSION" <<'PY'
import sys
current = tuple(int(part) for part in sys.argv[1].split("."))
target = tuple(int(part) for part in sys.argv[2].split("."))
sys.exit(0 if target > current else 1)
PY
then
  die "target version $VERSION is not greater than current $CURRENT"
fi

# Where the tag comes from, and whether it already exists somewhere.
REMOTE="${WAYFINDER_RELEASE_REMOTE:-origin}"
REMOTE_URL="$(git remote get-url "$REMOTE" 2>/dev/null)" || die "git remote '$REMOTE' does not exist"
[ -n "$REMOTE_URL" ] || die "git remote '$REMOTE' does not exist"
case "$REMOTE_URL" in
  *zaterka/wayfinder*) ;;
  *) log "warning: remote '$REMOTE' URL does not contain zaterka/wayfinder: $REMOTE_URL" ;;
esac
if git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null; then
  die "tag v$VERSION already exists locally"
fi
# A failed probe (network down, unknown remote) is treated as "unknown" and does
# not block; the push is the backstop. Exit 2 means no matching tag.
if git ls-remote --exit-code --tags "$REMOTE" "refs/tags/v$VERSION" >/dev/null 2>&1; then
  die "tag v$VERSION already exists on remote '$REMOTE'"
fi

if [ "$DRY_RUN" -eq 0 ]; then
  BRANCH="$(git rev-parse --abbrev-ref HEAD)"
  [ "$BRANCH" = "main" ] || die "releases are cut from main, not from '$BRANCH'"

  DIRTY="$(git status --porcelain --untracked-files=no)"
  [ -z "$DIRTY" ] || die "uncommitted tracked changes (commit or stash them first):
$DIRTY"

  log "checking main against $REMOTE"
  if ! git fetch --quiet "$REMOTE"; then
    log "warning: could not fetch '$REMOTE' (offline?); skipping the up-to-date check"
  else
    LOCAL_HEAD="$(git rev-parse HEAD)"
    REMOTE_MAIN="$(git rev-parse -q --verify "refs/remotes/$REMOTE/main" || true)"
    if [ -z "$REMOTE_MAIN" ]; then
      log "warning: '$REMOTE' has no main branch; skipping the up-to-date check"
    elif [ "$LOCAL_HEAD" != "$REMOTE_MAIN" ]; then
      die "HEAD ($LOCAL_HEAD) differs from $REMOTE/main ($REMOTE_MAIN); merge or rebase first, then re-run"
    else
      log "HEAD is up to date with $REMOTE/main"
    fi
  fi
fi

if [ "$DRY_RUN" -eq 1 ]; then
  log "dry run: no files will be modified"
  cat <<EOF
Would update:
  Cargo.toml                            version = "$CURRENT" -> version = "$VERSION"
  Cargo.lock                            wayfinder, wayfinder-core, wayfinder-bench -> $VERSION
  python/src/wayfinder_cli/__init__.py  __version__ = "$CURRENT" -> "$VERSION"
Would commit:  chore(release): $VERSION
Would tag:     v$VERSION (annotated, "wayfinder v$VERSION")
Not checked in a dry run: the branch, the working tree, and whether main matches $REMOTE/main.
EOF
  if [ "$SKIP_BUILD" -eq 0 ]; then
    echo "Would check:   cargo run --locked -p wayfinder -- --version == \"wayfinder $VERSION\""
  else
    echo "Would check:   (skipped via --skip-build; the release workflow still enforces it)"
  fi
  cat <<EOF
Publish (manual — the script never pushes):
  git push $REMOTE main
  git push $REMOTE v$VERSION
EOF
  exit 0
fi

# Rewrite exactly one version line per carrier; everything else stays byte-identical.
log "bumping version carriers to $VERSION"
if ! python3 - "$VERSION" <<'PY'
import re
import sys

target = sys.argv[1]
edits = []

def bump(path, pattern, replacement, what):
    with open(path, "r", encoding="utf-8", newline="") as f:
        text = f.read()
    matches = len(re.findall(pattern, text, flags=re.M))
    if matches != 1:
        sys.exit("error: %s: expected exactly 1 %s, found %d" % (path, what, matches))
    edits.append((path, re.sub(pattern, replacement, text, count=1, flags=re.M)))

bump(
    "Cargo.toml",
    r'^version = "[0-9]+\.[0-9]+\.[0-9]+"',
    'version = "%s"' % target,
    "version line",
)
bump(
    "python/src/wayfinder_cli/__init__.py",
    r'^__version__ = "[0-9]+\.[0-9]+\.[0-9]+"',
    '__version__ = "%s"' % target,
    "__version__ line",
)

with open("Cargo.lock", "r", encoding="utf-8", newline="") as f:
    lock = f.read()
for name in ("wayfinder", "wayfinder-core", "wayfinder-bench"):
    pattern = (
        r'^(\[\[package\]\]\nname = "%s"\nversion = ")[0-9]+\.[0-9]+\.[0-9]+"' % re.escape(name)
    )
    lock, count = re.subn(pattern, lambda m: m.group(1) + target + '"', lock, flags=re.M)
    if count != 1:
        sys.exit(
            "error: Cargo.lock: expected exactly 1 [[package]] block for %s, found %d"
            % (name, count)
        )
edits.append(("Cargo.lock", lock))

# Validate every carrier before writing any of them: no half-bumped tree.
for path, text in edits:
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)
PY
then
  die_recover_precommit "the version bump failed"
fi

# The bump must touch nothing but the three carriers.
log "checking blast radius"
CHANGED="$(git diff --name-only | LC_ALL=C sort)"
EXPECTED="$(printf '%s\n' "Cargo.lock" "Cargo.toml" "python/src/wayfinder_cli/__init__.py" | LC_ALL=C sort)"
[ "$CHANGED" = "$EXPECTED" ] || die_recover_precommit "the bump touched unexpected files:
$CHANGED"

if [ "$SKIP_BUILD" -eq 0 ]; then
  # --locked proves the hand-edited lockfile is exactly what cargo would write:
  # a stale entry fails here, before the version banner is even compiled. cargo
  # run (rather than target/debug) honours CARGO_TARGET_DIR and build.target-dir.
  log "building and reading the banner (debug, --locked)"
  if ! ACTUAL="$(cargo run --quiet --locked -p wayfinder -- --version)"; then
    die_recover_precommit "cargo run -p wayfinder failed"
  fi
  [ "$ACTUAL" = "wayfinder $VERSION" ] || die_recover_precommit "the freshly built binary printed '$ACTUAL', expected 'wayfinder $VERSION'"
  log "binary reports: $ACTUAL"
else
  log "skipping cargo checks (--skip-build); the release workflow still enforces them"
fi

log "committing and tagging"
git add Cargo.toml Cargo.lock python/src/wayfinder_cli/__init__.py
if ! git commit -m "chore(release): $VERSION"; then
  die_recover_precommit "git commit failed (check user.name and user.email)"
fi
if ! git tag -a "v$VERSION" -m "wayfinder v$VERSION"; then
  printf 'error: the release commit landed but the tag did not; finish with:\n  git tag -a v%s -m "wayfinder v%s"\n' "$VERSION" "$VERSION" >&2
  exit 1
fi
log "created annotated tag v$VERSION"

cat <<EOF
Publish it:
  git push $REMOTE main
  git push $REMOTE v$VERSION
EOF
