#!/usr/bin/env bash
# Hashes of the PE files a build ships, and why two builds differ. Used by reproducible.yml and release.yml.
#   repro.sh hashes DIST                       "<sha256>  <name>" for every .exe/.dll in DIST's 2017-18 ZIP and installer EXE
#   repro.sh why OLD.txt NEW.txt OLD_REF NEW_REF   markdown table of the files that differ, with the likely reason
# Run `why` inside a checkout that has both refs.
set -euo pipefail

hashes() {
  local dist=$1 z e
  for z in "$dist"/Forza-WCCF-2017-18-*.zip; do
    while IFS= read -r e; do
      printf '%s  %s\n' "$(unzip -p "$z" "$e" | sha256sum | cut -d' ' -f1)" "${e#*/}"
    done < <(unzip -Z1 "$z" | grep -iE '\.(exe|dll)$')
  done
  for e in "$dist"/Forza-WCCF-Installer-*.exe; do
    [ -e "$e" ] && printf '%s  %s\n' "$(sha256sum "$e" | cut -d' ' -f1)" Installer.exe
  done
  return 0
}

# the workspace package a shipped file is built from
pkg_of() {
  case $1 in
    Launcher.exe) echo launcher ;;
    overlay/forza_panel.dll) echo forza-panel ;;
    bin/wccfnet.dll) echo forza-wccfnet ;;
    bin/ICMP.DLL) echo forza-icmp ;;
    bin/dmath.dll) echo forza-detmath ;;
    bin/winmm.dll) echo forza-hook ;;
    bin/FPR_Emu.exe) echo forza-programs ;;
    Installer.exe) echo wccf-installer ;;
  esac
}

# the package and the workspace packages it depends on, as directories (cargo metadata --no-deps lists path dependencies)
dirs_of() {
  local meta=$1 todo=$2 seen=" "
  local n
  while [ -n "$todo" ]; do
    n=${todo%% *}
    todo=${todo#"$n"}
    todo=${todo# }
    case $seen in *" $n "*) continue ;; esac
    seen="$seen$n "
    jq -r --arg n "$n" '.packages[] | select(.name == $n) | .manifest_path' <<<"$meta" | sed -e 's|/Cargo.toml$||' -e "s|^$PWD/||"
    todo="$todo $(jq -r --arg n "$n" '.packages[] | select(.name == $n) | .dependencies[] | select(.path != null) | .name' <<<"$meta" | tr '\n' ' ')"
  done
}

why() {
  local old=$1 new=$2 oref=$3 nref=$4 meta name o n p d
  meta=$(RUSTUP_TOOLCHAIN=stable cargo metadata --no-deps --format-version 1)
  printf '| File | %s | %s | Reason |\n|---|---|---|---|\n' "$oref" "$nref"
  while read -r name; do
    o=$(awk -v f="$name" '$2 == f { print substr($1, 1, 12) }' "$old")
    n=$(awk -v f="$name" '$2 == f { print substr($1, 1, 12) }' "$new")
    [ "$o" = "$n" ] && continue
    if [ -z "$o" ]; then
      d="new file"
    elif [ -z "$n" ]; then
      d="removed"
    else
      p=$(pkg_of "$name")
      # shellcheck disable=SC2046
      if [ -z "$p" ] || ! git diff --quiet "$oref" "$nref" -- .cargo rust-toolchain.toml $(dirs_of "$meta" "$p"); then
        d="source changed"
      else
        d="no source change in $p or its workspace dependencies: toolchain or runner changed (or the build is not reproducible)"
      fi
    fi
    printf '| %s | %s | %s | %s |\n' "$name" "${o:--}" "${n:--}" "$d"
  done < <(cat "$old" "$new" | awk '{ print $2 }' | sort -u)
}

case ${1:-} in
  hashes) hashes "$2" | sort -k2 ;;
  why) why "$2" "$3" "$4" "$5" ;;
  *) echo "usage: repro.sh hashes DIST | why OLD.txt NEW.txt OLD_REF NEW_REF" >&2; exit 2 ;;
esac
