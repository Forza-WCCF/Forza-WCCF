#!/usr/bin/env bash
# The release candidate of a tag: a workflow_dispatch run of release.yml on the tagged commit whose files carry the
# tag's version. Used by release.yml on a tag push (job candidate). Needs GH_TOKEN and REPO.
#   candidate.sh find VERSION SHA     the newest successful candidate run's id on stdout, its dist left in cand-<id>/;
#                                     exit 2 when there is none, the reasons on stderr
#   candidate.sh verify DIR VERSION   every file in DIR is listed in SHA256SUMS.txt and matches it, and the files
#                                     carry VERSION (names, version.txt in the 2017-18 ZIP)
set -euo pipefail

# "<hash> *<name>" (Git Bash's sha256sum, binary mode) and "<hash>  <name>" give the same name
names() { awk '{ name = $2; sub(/^\*/, "", name); print name }' "$1"; }

find_candidate() {
  local v=$1 sha=$2 runs id skipped='' found=''
  # only a successful dispatch is a candidate: a build, a check or a virustotal job that failed is not one
  if ! runs=$(gh run list -R "$REPO" -w release.yml -e workflow_dispatch -c "$sha" -s success -L 20 --json databaseId --jq '.[].databaseId'); then
    echo "gh run list failed: cannot look for a candidate on $sha" >&2
    return 1
  fi
  for id in $runs; do
    # a run without a dist artifact (a failed build, an expired artifact) is reported below
    if ! gh run download "$id" -R "$REPO" -n dist -D "cand-$id" >&2; then skipped="$skipped $id"; continue; fi
    if names "cand-$id/SHA256SUMS.txt" | grep -xF "Forza-WCCF-2017-18-$v.zip" >/dev/null; then echo "$id"; return 0; fi
    found="$found $(names "cand-$id/SHA256SUMS.txt" | grep -E '^Forza-WCCF-2017-18-.*\.zip$' | tr '\n' ' ' || true)"
  done
  echo "no candidate with version=$v on $sha (only a successful dispatch counts)" >&2
  if [ -n "${found// /}" ]; then echo "the candidates on this commit carry other versions:$found" >&2; fi
  if [ -n "$skipped" ]; then echo "the dist artifact could not be downloaded from run(s):$skipped" >&2; fi
  return 2
}

verify() {
  local dir=$1 v=$2 f
  # the draft uploads every file in dist/, so nothing may be there unlisted
  if ! diff <(cd "$dir" && find . -maxdepth 1 -type f ! -name SHA256SUMS.txt -printf '%f\n' | LC_ALL=C sort) \
            <(names "$dir/SHA256SUMS.txt" | LC_ALL=C sort) >&2; then
    echo "$dir: the files and SHA256SUMS.txt list different names (< files only, > sums only)" >&2
    return 1
  fi
  if ! (cd "$dir" && sha256sum --quiet --strict -c SHA256SUMS.txt) >&2; then
    echo "$dir: files do not match SHA256SUMS.txt" >&2
    return 1
  fi
  for f in "Forza-WCCF-2017-18-$v.zip" "Forza-WCCF-Installer-$v-windows-x64.exe"; do
    [ -f "$dir/$f" ] || { echo "$dir: $f is missing" >&2; return 1; }
  done
  f=$(unzip -p "$dir/Forza-WCCF-2017-18-$v.zip" Forza-WCCF-2017-18/version.txt | tr -d '\r\n')
  [ "$f" = "kit-$v" ] || { echo "$dir: version.txt reads '$f', not kit-$v" >&2; return 1; }
}

case "${1:-}" in
  find) find_candidate "$2" "$3" ;;
  verify) verify "$2" "$3" ;;
  *) echo "usage: candidate.sh find VERSION SHA | verify DIR VERSION" >&2; exit 64 ;;
esac
