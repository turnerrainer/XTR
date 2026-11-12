#!/bin/sh
# Download the pinned Xerces jar and check its SHA-256.
# Usage: fetch-xerces.sh [CACHE_DIR]
# A wrong checksum deletes the file and ends with exit code 1.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
. "$here/pins.env"
cache=${1:-${XTR_ORACLE_CACHE:-$here/.cache}}
mkdir -p "$cache"
jar="$cache/xercesImpl-$XERCES_VERSION.jar"

sha256() {
  f=$1
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$f" | cut -d' ' -f1
  else shasum -a 256 "$f" | cut -d' ' -f1; fi
}

if [ ! -f "$jar" ]; then
  tmp="$jar.part"
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
    --output "$tmp" "$XERCES_URL"
  mv "$tmp" "$jar"
fi
actual=$(sha256 "$jar")
if [ "$actual" != "$XERCES_SHA256" ]; then
  echo "fetch-xerces: SHA-256 mismatch for the Xerces jar" >&2
  rm -f "$jar"
  exit 1
fi
echo "$jar"
