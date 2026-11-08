#!/usr/bin/env bash
# The release candidate of a tag: a workflow_dispatch run of release.yml on the tagged commit whose build succeeded and
# whose files carry the tag's version. Used by release.yml on a tag push (job candidate). Needs GH_TOKEN and REPO.
#   candidate.sh find VERSION SHA     the newest candidate run's id on stdout, its dist left in cand-<id>/;
#                                     exit 2 when there is none, the reasons on stderr
#   candidate.sh verify DIR VERSION   every file in DIR is listed in SHA256SUMS.txt and matches it, and the files
#                                     carry VERSION (names, version.txt in the 2017-18 ZIP)
set -euo pipefail

# "<hash> *<name>" (Git Bash's sha256sum, binary mode) and "<hash>  <name>" give the same name
names() { awk '{ name = $2; sub(/^\*/, "", name); print name }' "$1"; }

find_candidate() {
  local v=$1 sha=$2 runs id build skipped='' found=''
  # The dispatch's build job decides, not the run's conclusion: a failed virustotal job (a Microsoft false positive)
  # leaves the files its build made intact, and the tag run's own virustotal job judges them again.
  if ! runs=$(gh run list -R "$REPO" -w release.yml -e workflow_dispatch -c "$sha" -L 20 --json databaseId --jq '.[].databaseId'); then
    echo "gh run list failed: cannot look for a candidate on $sha" >&2
    return 1
  fi
  for id in $runs; do
    if ! build=$(gh run view "$id" -R "$REPO" --json jobs --jq '[.jobs[] | select(.name == "build")][0].conclusion // ""'); then
      echo "gh run view failed: cannot read the jobs of run $id" >&2
      return 1
    fi
    if [ "$build" != success ]; then skipped="$skipped $id (build: ${build:-missing or still running})"; continue; fi
    # an expired dist artifact is reported below
    if ! gh run download "$id" -R "$REPO" -n dist -D "cand-$id" >&2; then skipped="$skipped $id (no dist artifact)"; continue; fi
    if names "cand-$id/SHA256SUMS.txt" | grep -xF "Forza-WCCF-2017-18-$v.zip" >/dev/null; then echo "$id"; return 0; fi
    found="$found $(names "cand-$id/SHA256SUMS.txt" | grep -E '^Forza-WCCF-2017-18-.*\.zip$' | tr '\n' ' ' || true)"
  done
  echo "no candidate with version=$v on $sha (only a dispatch whose build succeeded counts)" >&2
  if [ -n "${found// /}" ]; then echo "the candidates on this commit carry other versions:$found" >&2; fi
  if [ -n "$skipped" ]; then echo "runs on this commit without a candidate:$skipped" >&2; fi
  return 2
}

verify() {
  local dir=$1 v=$2 f
  [ -f "$dir/SHA256SUMS.txt" ] || { echo "$dir: SHA256SUMS.txt is missing" >&2; return 1; }
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
  f=$(unzip -p "$dir/Forza-WCCF-2017-18-$v.zip" Forza-WCCF-2017-18/version.txt | tr -d '\r\n') || {
    echo "$dir: the 2017-18 ZIP has no Forza-WCCF-2017-18/version.txt" >&2; return 1; }
  [ "$f" = "kit-$v" ] || { echo "$dir: version.txt reads '$f', not kit-$v" >&2; return 1; }
}

case "${1:-}" in
  find) find_candidate "$2" "$3" ;;
  verify) verify "$2" "$3" ;;
  *) echo "usage: candidate.sh find VERSION SHA | verify DIR VERSION" >&2; exit 64 ;;
esac
