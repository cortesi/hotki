#!/usr/bin/env bash
# Dump macOS private SkyLight framework info to stdout, robustly.
#
# Usage: bash dump_skylight_v2.sh [--no-extract | --force-extract] [--keep-tmp]
#   --no-extract     Do not extract from the dyld cache even if needed
#   --force-extract  Always extract from the dyld cache (preferred on Big Sur+)
#   --keep-tmp       Keep temp directory (prints path at end)
#
# Exit codes:
#   0 success
#   1 general error
#   2 dependency missing / cannot obtain binary

set -uo pipefail

# ---------------- flags
AUTO_EXTRACT="auto"   # auto|never|always
KEEP_TMP="no"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-extract)    AUTO_EXTRACT="never"; shift;;
    --force-extract) AUTO_EXTRACT="always"; shift;;
    --keep-tmp)      KEEP_TMP="yes"; shift;;
    -h|--help) sed -n '1,120p' "$0"; exit 0;;
    *) echo "Unknown option: $1" >&2; exit 2;;
  esac
done

# ---------------- utils
die() { echo "ERROR: $*" >&2; exit 2; }
log() { printf '[*] %s\n' "$*" >&2; }
sep() { printf '\n===== %s =====\n' "$*"; }
run() { printf '\n# %s\n' "$*"; eval "$*"; }

have() { command -v "$1" >/dev/null 2>&1; }

find_tool() {
  # $1: comma-separated list of candidate tool names (in order of preference)
  local IFS=','; local name; for name in $1; do
    # 1) PATH lookup
    if command -v "$name" >/dev/null 2>&1; then command -v "$name"; return 0; fi
    # 2) xcrun lookup
    if have xcrun; then
      local p; p="$(xcrun -f "$name" 2>/dev/null || true)"
      [[ -n "${p:-}" && -x "$p" ]] && { printf '%s\n' "$p"; return 0; }
    fi
    # 3) common brew prefixes
    for dir in /opt/homebrew/bin /usr/local/bin /usr/local/sbin; do
      [[ -x "$dir/$name" ]] && { printf '%s\n' "$dir/$name"; return 0; }
    done
  done
  return 1
}

OS_VER="$(/usr/bin/sw_vers -productVersion 2>/dev/null || echo unknown)"
ARCH="$(uname -m 2>/dev/null || echo unknown)"

TMPDIR="$(/usr/bin/mktemp -d -t skylightdump.XXXXXX)"
cleanup() {
  if [[ "$KEEP_TMP" == "yes" ]]; then log "Keeping temp dir: $TMPDIR"; else rm -rf "$TMPDIR"; fi
}
trap cleanup EXIT

# ---------------- resolve tools
OTOOL="$(find_tool otool || true)"
NM="$(find_tool nm || true)"
CODESIGN="$(find_tool codesign || true)"
PLUTIL="$(find_tool plutil || true)"
DYLDINFO="$(find_tool dyldinfo || true)"
SWDEMANGLER="$(find_tool swift-demangle || true)"
CLASSDUMP="$(find_tool class-dump,class-dump-z,objc-class-dump || true)"
DYLDUTIL="$(find_tool dyld_shared_cache_util || true)"

# ---------------- SDK (.tbd) lookup for linkable exports
find_sdk_root() {
  local sdk
  if have xcrun; then
    sdk="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
  fi
  if [[ -z "${sdk:-}" || ! -d "$sdk" ]]; then
    for d in /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk \
             /Library/Developer/CommandLineTools/SDKs/MacOSX*.sdk; do
      [[ -d "$d" ]] && { sdk="$d"; break; }
    done
  fi
  [[ -d "${sdk:-}" ]] && echo "$sdk"
}

SDK_ROOT="$(find_sdk_root || true)"
find_sdk_tbd() {
  local sdk="${SDK_ROOT:-}"
  [[ -d "$sdk" ]] || return 1
  local p
  for p in \
    "$sdk/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight.tbd" \
    "$sdk/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight.tbd"; do
    [[ -f "$p" ]] && { echo "$p"; return 0; }
  done
  return 1
}
SDK_TBD="$(find_sdk_tbd || true)"

# ---------------- find dyld cache
dyld_cache_for_arch() {
  # Try modern Cryptex path first (macOS 12+), then legacy path
  local cdirs=(
    "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld"
    "/System/Library/dyld"
  )
  local cdir n
  for cdir in "${cdirs[@]}"; do
    case "$ARCH" in
      arm64* )
        for n in dyld_shared_cache_arm64e dyld_shared_cache_arm64; do [[ -f "$cdir/$n" ]] && { echo "$cdir/$n"; return; } done;;
      x86_64* )
        for n in dyld_shared_cache_x86_64h dyld_shared_cache_x86_64; do [[ -f "$cdir/$n" ]] && { echo "$cdir/$n"; return; } done;;
    esac
    # Any cache at all
    if ls -1 "$cdir"/dyld_shared_cache_* >/dev/null 2>&1; then
      ls -1 "$cdir"/dyld_shared_cache_* 2>/dev/null | head -n1
      return
    fi
  done
}
CACHE_PATH="$(dyld_cache_for_arch || true)"

# ---------------- locate SkyLight on disk (try several canonical paths)
CANDIDATES=(
  "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight"
  "/System/Library/PrivateFrameworks/SkyLight.framework/Versions/Current/SkyLight"
  "/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight"
  "/System/Library/StagedFrameworks/SkyLight.framework/SkyLight"
)

find_on_disk() {
  local p
  for p in "${CANDIDATES[@]}"; do
    [[ -f "$p" ]] && { echo "$p"; return 0; }
  done
  return 1
}

BIN_ON_DISK="$(find_on_disk || true)"

# ---------------- helpers
has_meaningful_symbols() {
  local bin="$1"
  [[ -n "${NM:-}" && -f "$bin" ]] || return 1
  "$NM" -gjU "$bin" 2>/dev/null | wc -l | awk '{exit ($1>50)?0:1}'
}

objc_metadata_present() {
  local bin="$1"
  [[ -n "${OTOOL:-}" && -f "$bin" ]] || return 1
  "$OTOOL" -l "$bin" 2>/dev/null | grep -q "__objc_classlist"
}

list_cache_for_skylight() {
  [[ -n "${DYLDUTIL:-}" && -f "${CACHE_PATH:-/dev/null}" ]] || return 1
  "$DYLDUTIL" -list "$CACHE_PATH" 2>/dev/null | grep -i "SkyLight\.framework/.*/SkyLight" || true
}

extract_from_cache() {
  # tries to extract just SkyLight; falls back to full extract
  [[ -n "${DYLDUTIL:-}" && -f "${CACHE_PATH:-/dev/null}" ]] || return 1
  local OUTDIR="$TMPDIR/extract"; mkdir -p "$OUTDIR"
  local img
  img="$(list_cache_for_skylight | head -n1 | awk '{print $1}' || true)"
  if [[ -n "$img" ]]; then
    log "Attempting targeted cache extract of: $img"
    # Try a few arg styles; different OS builds vary
    "$DYLDUTIL" -extract "$OUTDIR" -cache "$CACHE_PATH" -image "$img" >/dev/null 2>&1 || \
    "$DYLDUTIL" -image "$img" -extract "$OUTDIR" "$CACHE_PATH" >/dev/null 2>&1 || \
    "$DYLDUTIL" -extract "$OUTDIR" "$CACHE_PATH" >/dev/null 2>&1 || true
  else
    log "SkyLight not listed explicitly; extracting whole cache (slow)..."
    "$DYLDUTIL" -extract "$OUTDIR" "$CACHE_PATH" >/dev/null 2>&1 || true
  fi
  /usr/bin/find "$OUTDIR" -type f -path "*/SkyLight.framework/*/SkyLight" -print -quit 2>/dev/null || true
}

choose_binary() {
  # returns path to a *real* file with symbols if available
  if [[ -n "${BIN_ON_DISK:-}" && -f "$BIN_ON_DISK" ]]; then
    if has_meaningful_symbols "$BIN_ON_DISK"; then echo "$BIN_ON_DISK"; return 0; fi
    [[ "$AUTO_EXTRACT" == "never" ]] && { echo "$BIN_ON_DISK"; return 0; }
  fi
  case "$AUTO_EXTRACT" in
    always|auto)
      local ex; ex="$(extract_from_cache || true)"
      [[ -n "$ex" && -f "$ex" ]] && { echo "$ex"; return 0; }
      ;;
  esac
  # As a last resort, if we have an on-disk stub, return it (better than nothing)
  [[ -n "${BIN_ON_DISK:-}" && -f "$BIN_ON_DISK" ]] && { echo "$BIN_ON_DISK"; return 0; }
  # No binary at all
  echo ""
  return 1
}

BIN="$(choose_binary || true)"

# ---------------- header
sep "SkyLight Dump (v3)"
printf "Date: %s\n" "$(date)"
printf "macOS: %s\n" "$OS_VER"
printf "Arch: %s\n" "$ARCH"
printf "On-disk candidates checked:\n"
printf '  - %s\n' "${CANDIDATES[@]}"
[[ -n "${CACHE_PATH:-}" ]] && printf "dyld cache: %s\n" "$CACHE_PATH"
[[ -n "${SDK_ROOT:-}" ]] && printf "SDK root: %s\n" "$SDK_ROOT"
[[ -n "${SDK_TBD:-}" ]] && printf "SDK stub: %s\n" "$SDK_TBD"
printf "Selected binary: %s\n" "${BIN:-<none>}"

# ---------------- linkable exports from SDK (.tbd) and Rust skeleton
if [[ -f "${SDK_TBD:-}" ]]; then
  sep "Linkable Exports (.tbd)"
  printf "Stub: %s\n" "$SDK_TBD"
  printf "Install name: %s\n" "$(awk -F': ' '/^install-name:/ {print $2; exit}' "$SDK_TBD" | sed 's/^\"//; s/\"$//')"

  sep "Rust Extern Skeleton"
  if command -v uv >/dev/null 2>&1; then
    # Use PyYAML for robust parsing of .tbd and generate a clean Rust FFI skeleton
    uv run --quiet --with pyyaml python - <<'PY' 2>/dev/null || true
import os, re, sys, textwrap
try:
    import yaml
except Exception as e:
    print('# NOTE: PyYAML unavailable; falling back to naive parsing elsewhere.')
    sys.exit(0)

tbd_path = os.environ.get('SDK_TBD')
if not tbd_path or not os.path.isfile(tbd_path):
    sys.exit(0)

with open(tbd_path, 'r', encoding='utf-8') as f:
    data = f.read()

# Remove the document tag so SafeLoader accepts it
data = re.sub(r'^---\s*!tapi-tbd\s*', '---', data, count=1, flags=re.M)
doc = yaml.safe_load(data)
exports = doc.get('exports', []) if isinstance(doc, dict) else []
syms = set()
for e in exports:
    for s in e.get('symbols', []) or []:
        if isinstance(s, str):
            syms.add(s)

# Categorize
objc_classes = []
constants = []
funcs = []
swift = []
others = []

for s in sorted(syms):
    if not s.startswith('_'):
        continue
    if s.startswith('_OBJC_CLASS_$_'):
        objc_classes.append(s)
    elif s.startswith('_k'):
        constants.append(s)
    elif s.startswith('_$s'):
        swift.append(s)
    else:
        name = s[1:]
        # Keep C-style identifiers (no dots/dollars), focus on SkyLight/CGS APIs
        if re.match(r'^(CGS|SL|SLS)[A-Za-z0-9_]*$', name):
            funcs.append(s)
        else:
            others.append(s)

def rust_header():
    print('use core::ffi::c_void;')
    print('')
    print('#[link(name = "SkyLight", kind = "framework")]')
    print('extern "C" {')

def rust_footer():
    print('}')

rust_header()

# Functions
if funcs:
    print('    // Functions (signatures unknown — fill in as needed):')
    for s in funcs:
        print(f'    pub fn {s[1:]}();')
    print('')

# Constants
if constants:
    print('    // Constants')
    for s in constants:
        nm = s[1:]
        print(f'    #[link_name = "{nm}"]')
        print(f'    pub static {nm}: *const c_void;')
    print('')

# Objective-C classes
if objc_classes:
    print('    // Objective-C classes (use objc runtime to work with these)')
    for s in objc_classes:
        cls = s[len('_OBJC_CLASS_$_'):]
        rust_ident = 'OBJC_CLASS__' + cls
        print(f'    #[link_name = "OBJC_CLASS_$_{cls}"]')
        print(f'    pub static {rust_ident}: *const c_void;')
    print('')

rust_footer()

# Optional: list Swift-mangled names and others for reference
if swift:
    print('\n# Swift-mangled exports (reference only):')
    for s in swift[:50]:
        print(s)
    if len(swift) > 50:
        print(f'# ... and {len(swift)-50} more')

if others:
    print('\n# Other exports not matching CGS/SL/SLS (reference only):')
    for s in others[:50]:
        print(s)
    if len(others) > 50:
        print(f'# ... and {len(others)-50} more')
PY
  else
    printf "# uv not found; showing raw stub symbols instead.\n"
    awk '/^[[:space:]]*symbols:/, /\]/ { print }' "$SDK_TBD" | sed "s/'//g" | grep -oE '_[A-Za-z0-9_.$]+' | sort -u
  fi
fi

if [[ -z "${BIN:-}" || ! -f "$BIN" ]]; then
  log "Could not obtain a standalone SkyLight binary. Falling back to cache heuristics."
  log "Tips: install Xcode CLT for 'dyld_shared_cache_util', or re-run with --force-extract on systems that have it."

  # Fallback: print the cache map entry for SkyLight (addresses give a sense of size/segments)
  if [[ -f "${CACHE_PATH:-}" ]]; then
    MAPFILE="${CACHE_PATH}.map"
    if [[ -f "$MAPFILE" ]]; then
      sep "Cache Map Entry (SkyLight)"
      awk 'BEGIN{p=0} 
           tolower($0) ~ /\/skylight\.framework\/.*\/skylight$/ {print; p=5; next} 
           p>0 {print; p--}' "$MAPFILE"
    fi
    # Heuristic symbol sweep across cache for likely CGS/SL exports
    if command -v strings >/dev/null 2>&1; then
      sep "Heuristic Symbols From Cache (CGS/SL prefixes)"
      # Order of pipeline chosen so head short-circuits early without sorting whole cache output
      run "strings -a '$CACHE_PATH' | egrep '^(CGS\\w+|SL\\w+)$' | head -n 4000 | sort -u"
      sep "Note"
      cat <<'EOF'
# No standalone SkyLight binary was available and dyld cache extraction tools
# were not found. The above list is derived heuristically from the cache and
# may include noise from other images. Install Xcode Command Line Tools to get
# dyld_shared_cache_util for high‑fidelity extraction.
EOF
      exit 0
    fi
  fi
  exit 2
fi

# ---------------- Info.plist (if accessible)
FW_DIR="$(dirname "$(dirname "$BIN")")"   # .../Versions/A
INFO1="$FW_DIR/Resources/Info.plist"
INFO2="$(dirname "$FW_DIR")/Resources/Info.plist"
if [[ -n "${PLUTIL:-}" ]]; then
  for P in "$INFO1" "$INFO2"; do
    if [[ -f "$P" ]]; then
      sep "Info.plist ($P)"
      run "$PLUTIL -p '$P'"
    fi
  done
fi

# ---------------- code signature
if [[ -n "${CODESIGN:-}" ]]; then
  sep "Code Signature"
  run "$CODESIGN -dv --verbose=4 '$BIN' 2>&1 | sed 's/^/  /'"
fi

# ---------------- Mach-O header & loads
if [[ -n "${OTOOL:-}" ]]; then
  sep "Mach-O Header"
  run "$OTOOL -hv '$BIN'"
  sep "Load Commands"
  run "$OTOOL -l '$BIN'"
  sep "Linked Libraries"
  run "$OTOOL -L '$BIN'"
fi

# ---------------- exports / dyldinfo
if [[ -n "${DYLDINFO:-}" ]]; then
  sep "dyldinfo Exports"
  run "$DYLDINFO -export '$BIN' 2>/dev/null || $DYLDINFO -exports '$BIN' 2>/dev/null || true"
fi

# ---------------- symbols
if [[ -n "${NM:-}" ]]; then
  sep "Exported Symbols (global, names only)"
  run "$NM -gjU '$BIN' 2>/dev/null | sort"
  sep "Exported Symbols (with kinds/addresses)"
  run "$NM -gU '$BIN' 2>/dev/null | sort"
  sep "Likely CGS/SkyLight API (name filter)"
  run "$NM -gjU '$BIN' 2>/dev/null | egrep '(^|_)((CGS|SL)\\w+)$' | sort"
  if [[ -n "${SWDEMANGLER:-}" ]] && "$NM" -gjU "$BIN" 2>/dev/null | grep -q '^_\\$s'; then
    sep "Swift Symbols (demangled)"
    run "$NM -gjU '$BIN' 2>/dev/null | '$SWDEMANGLER' -compact"
  fi
fi

# ---------------- Objective-C metadata
if [[ -n "${OTOOL:-}" ]]; then
  sep "Objective-C Metadata (otool -oV)"
  run "$OTOOL -oV '$BIN' 2>/dev/null || true"
fi

# ---------------- class-dump (only if present AND ObjC metadata exists)
if [[ -n "${CLASSDUMP:-}" ]]; then
  if objc_metadata_present "$BIN"; then
    sep "class-dump (best-effort ObjC headers)"
    run "'$CLASSDUMP' '$BIN' 2>/dev/null || true"
  else
    sep "class-dump"
    printf "# ObjC metadata not detected; skipping class-dump.\n"
  fi
else
  sep "class-dump"
  printf "# class-dump not found. Install one of: class-dump, class-dump-z, objc-class-dump.\n"
fi

# ---------------- Swift sections (if any)
if [[ -n "${OTOOL:-}" ]]; then
  for SEC in __swift5_types __swift5_proto __swift5_protos __swift5_typeref __swift5_fieldmd __swift5_builtin; do
    if "$OTOOL -s __TEXT $SEC '$BIN'" >/dev/null 2>&1; then
      sep "Swift Section: $SEC"
      run "$OTOOL -s __TEXT $SEC '$BIN'"
    fi
  done
fi

# ---------------- notes about cache/thin binaries
if ! has_meaningful_symbols "$BIN"; then
  sep "Note"
  cat <<'EOF'
# The selected binary appears to have very few exported symbols.
# On modern macOS, SkyLight is usually cache-only. For deeper introspection,
# re-run with --force-extract (recommended on Big Sur and later).
EOF
fi

if [[ "$KEEP_TMP" == "yes" ]]; then
  sep "Temp Directory"
  printf "%s\n" "$TMPDIR"
fi
