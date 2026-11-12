#!/bin/sh
# Run the Xerces oracle in a container without network.
# Usage: run-oracle.sh SCHEMA_DIR MESSAGE_DIR   (JSON on stdout)
#        run-oracle.sh --info
# Environment: XTR_ORACLE_CACHE (jar directory, default: .cache next to this script).
# Exit codes: 0 ok, 10 Docker or jar not available (the caller may skip),
#             1 the jar has the wrong SHA-256 or the oracle failed.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
. "$here/pins.env"
cache=${XTR_ORACLE_CACHE:-$here/.cache}
jar="$cache/xercesImpl-$XERCES_VERSION.jar"

sha256() {
  f=$1
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$f" | cut -d' ' -f1
  else shasum -a 256 "$f" | cut -d' ' -f1; fi
}

command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1 || { echo "run-oracle: Docker is not available" >&2; exit 10; }
[ -f "$jar" ] || { echo "run-oracle: Xerces jar is missing; run fetch-xerces.sh" >&2; exit 10; }
if [ "$(sha256 "$jar")" != "$XERCES_SHA256" ]; then
  echo "run-oracle: SHA-256 mismatch for the Xerces jar" >&2
  exit 1
fi
if ! docker image inspect "$ORACLE_IMAGE" >/dev/null 2>&1; then
  docker pull --quiet "$ORACLE_IMAGE" >/dev/null || { echo "run-oracle: cannot pull the image" >&2; exit 10; }
fi

mounts="-v $here:/oracle:ro -v $cache:/lib-xerces:ro"
if [ "${1:-}" = "--info" ]; then
  args="--info"
else
  [ $# -eq 2 ] || { echo "usage: run-oracle.sh SCHEMA_DIR MESSAGE_DIR" >&2; exit 1; }
  xsd=$(cd "$1" && pwd)
  msg=$(cd "$2" && pwd)
  mounts="$mounts -v $xsd:/schema:ro -v $msg:/messages:ro"
  args="/schema /messages"
fi

# shellcheck disable=SC2086
exec docker run --rm --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges --pids-limit 256 --memory 1g \
  --tmpfs /work:rw,size=64m,exec $mounts "$ORACLE_IMAGE" \
  sh -c 'javac -nowarn -d /work -cp /lib-xerces/xercesImpl-'"$XERCES_VERSION"'.jar /oracle/Oracle.java && exec java -Xmx512m -cp /work:/lib-xerces/xercesImpl-'"$XERCES_VERSION"'.jar Oracle '"$args"
