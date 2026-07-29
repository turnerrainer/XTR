#!/usr/bin/env bash
# harvest-xtee-wsdls.sh — vendor Estonian X-Road WSDLs into wsdl/
#
# Fetches RIA's public catalog (https://x-tee.ee/catalogue-data/EE/
# index.json) and downloads every WSDL published there, organising
# by subsystem, with per-WSDL .meta.yaml sidecars that trigger
# XTR's X-Road envelope wrapping.
#
# WARNING: as of this script's writing the EE catalog has ~421
# subsystems / ~637 unique WSDLs / ~4300 methods. Ingesting all of
# them into XTR generates ~3000+ DSL files (~50 MB), and the
# current DSL loader validates every Handlebars template at boot
# — 3000+ templates takes MINUTES. See the "Cap what you ingest"
# section below.
#
# Usage:
#   ./scripts/harvest-xtee-wsdls.sh                    # fetch all
#   ./scripts/harvest-xtee-wsdls.sh --subsystem rr     # just RR
#   ./scripts/harvest-xtee-wsdls.sh --subsystem rr,liiklusregister
#   ./scripts/harvest-xtee-wsdls.sh --member 70003098  # by member
#   ./scripts/harvest-xtee-wsdls.sh --member 70001231,70003098,70008658,70009445
#
# When both --subsystem and --member are given, both filters apply.
#
# Requires: bash, curl, python3.

set -eu

CATALOG_URL="https://x-tee.ee/catalogue-data/EE/index.json"
CATALOG_BASE="https://x-tee.ee/catalogue-data/EE"
OUT_DIR="wsdl"
SUBSET=""
MEMBERS=""

while [[ $# -gt 0 ]]; do
  case $1 in
    --subsystem) SUBSET="$2"; shift 2 ;;
    --member)    MEMBERS="$2"; shift 2 ;;
    --out)       OUT_DIR="$2"; shift 2 ;;
    -h|--help)
      grep '^# ' "$0" | sed 's/^# //'
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "==> fetching catalog: $CATALOG_URL"
curl -sfSL --max-time 30 -o "$work/index.json" "$CATALOG_URL"
subs=$(python3 -c "
import json, sys
d = json.load(open('$work/index.json'))
subset = '$SUBSET'
members = '$MEMBERS'
wanted_sub = set(s.strip() for s in subset.split(',') if s.strip())
wanted_mem = set(s.strip() for s in members.split(',') if s.strip())
for s in d:
    if not s.get('methods'):
        continue
    if wanted_sub and s['subsystemCode'] not in wanted_sub:
        continue
    if wanted_mem and s['memberCode'] not in wanted_mem:
        continue
    print(f\"{s['memberClass']}\t{s['memberCode']}\t{s['subsystemCode']}\")
")
n=$(echo "$subs" | grep -c . || true)
echo "==> $n subsystem(s) selected"
if [[ $n -eq 0 ]]; then
  echo "nothing to fetch" >&2
  exit 1
fi

# Build the WSDL fetch plan + subsystemCode dir mapping.
python3 <<PYEOF > "$work/plan.tsv"
import json, collections, re
d = json.load(open('$work/index.json'))
subset = '$SUBSET'
members = '$MEMBERS'
wanted_sub = set(s.strip() for s in subset.split(',') if s.strip())
wanted_mem = set(s.strip() for s in members.split(',') if s.strip())
subs = [
    s for s in d
    if s.get('methods')
    and (not wanted_sub or s['subsystemCode'] in wanted_sub)
    and (not wanted_mem or s['memberCode'] in wanted_mem)
]
code_count = collections.Counter(s['subsystemCode'] for s in subs)
def dir_for(s):
    code = re.sub(r'[^a-zA-Z0-9._-]', '-', s['subsystemCode'])
    if code_count[s['subsystemCode']] > 1:
        code = f"{code}-{s['memberCode']}"
    return code
for s in subs:
    g = dir_for(s)
    for w in sorted({m['wsdl'] for m in s['methods'] if m.get('wsdl')}):
        # cols: url-rel-path, group-dir, member-class, member-code, subsystem-code
        print(f"{w}\t{g}\t{s['memberClass']}\t{s['memberCode']}\t{s['subsystemCode']}")
PYEOF

wsdl_count=$(wc -l < "$work/plan.tsv")
echo "==> $wsdl_count unique WSDL(s) to fetch"

mkdir -p "$OUT_DIR"
echo "==> parallel fetch (20 concurrent)..."
awk -F'\t' -v base="$CATALOG_BASE" -v out="$OUT_DIR" '
{
  fname = $1
  sub(".*/", "", fname)
  printf "%s/%s|%s/%s/%s|%s|%s|%s\n", base, $1, out, $2, fname, $3, $4, $5
}' "$work/plan.tsv" > "$work/fetch.txt"

mkdir -p $(awk -F'|' '{print $2}' "$work/fetch.txt" | xargs -I {} dirname {} | sort -u)

fail=0
xargs -P 20 -I {} bash -c '
  IFS="|" read url out mclass mcode subcode <<< "{}"
  mkdir -p "$(dirname "$out")"
  if ! curl -sfSL --max-time 30 -o "$out" "$url" 2>/dev/null; then
    echo "FAIL $url" >&2
    exit 0
  fi
  meta="${out%.wsdl}.meta.yaml"
  cat > "$meta" <<META
member_class: $mclass
member_code: "$mcode"
subsystem_code: $subcode
META
' < "$work/fetch.txt"

echo "==> done. summary:"
echo "    WSDL files:  $(find "$OUT_DIR" -name '*.wsdl' | wc -l)"
echo "    meta files:  $(find "$OUT_DIR" -name '*.meta.yaml' | wc -l)"
echo "    total size:  $(du -sh "$OUT_DIR" | cut -f1)"
echo
echo "Next steps:"
echo "  1. Verify: ls $OUT_DIR/"
echo "  2. Boot XTR: docker compose up   (or 'cargo run')"
echo "  3. See /api for the endpoint list."
echo
echo "Cap what you ingest — boot time notes:"
echo "  * ~ 5 subsystems  -> instant boot"
echo "  * ~ 20 subsystems -> few seconds"
echo "  * ALL subsystems (~421) -> loader takes MANY MINUTES because"
echo "    every Handlebars template is validated at boot. If you"
echo "    only need specific services, use --subsystem to filter."
