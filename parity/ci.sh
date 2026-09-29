#!/usr/bin/env bash
# Compare this build with Python repoglass; exit non-zero on any difference.
#
#   PY_RPG=... RS_RPG=... parity/ci.sh NAME=DIR [NAME=DIR ...]
#
# Each DIR is copied to $RPG_WORK/NAME and indexed by both. Every corpus
# gets the database, output and cross-read checks; the first also gets
# the settings variants, lifecycle and lock checks. Queries come from
# queries.json, keyed by NAME.
set -euo pipefail
: "${PY_RPG:?Python rpg}" "${RS_RPG:?Rust rpg}"
here=$(cd "$(dirname "$0")" && pwd)
python=$(dirname "$PY_RPG")/python
export RPG_WORK=${RPG_WORK:-$(mktemp -d)} RPG_SCRATCH=${RPG_SCRATCH:-$(mktemp -d)}
export RPG_QUERIES=${RPG_QUERIES:-$here/queries.json}

first=""
for spec in "$@"; do
  name=${spec%%=*}
  src=${spec#*=}
  rm -rf "${RPG_WORK:?}/$name"
  mkdir -p "$RPG_WORK/$name"
  tar -C "$src" --exclude=.git --exclude=target --exclude=.venv --exclude=__pycache__ -cf - . \
    | tar -C "$RPG_WORK/$name" -xf -
  for side in py rs; do
    home=$RPG_SCRATCH/home-$side-$name
    rm -rf "$home"
    binary=$PY_RPG; [ $side = rs ] && binary=$RS_RPG
    (cd "$RPG_WORK/$name" && REPOGLASS_HOME=$home "$binary" index >/dev/null)
  done
  echo "== $name"
  "$python" "$here/parity.py" "$RPG_SCRATCH"/home-py-"$name"/index/*/index.db \
                              "$RPG_SCRATCH"/home-rs-"$name"/index/*/index.db
  "$python" "$here/outputs.py" "$name" "$RPG_SCRATCH/home-py-$name" "$RPG_SCRATCH/home-rs-$name"
  "$python" "$here/cross.py" "$name" "$RPG_SCRATCH/home-py-$name" default
  [ -z "$first" ] && first=$name
done

echo "== settings variants on $first"
"$python" "$here/variants.py" "$first"
export RPG_SMALL_TREE=$RPG_WORK/$first
echo "== lifecycle"
"$python" "$here/life.py"
echo "== held lock"
"$python" "$here/lock.py"
