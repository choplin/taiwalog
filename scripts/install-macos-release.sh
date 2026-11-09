#!/bin/sh
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
  echo "This release bundle supports macOS only." >&2
  exit 1
fi

case "$#" in
  0) prefix=/usr/local ;;
  1)
    if [ -z "$1" ]; then
      echo "The install prefix must not be empty." >&2
      exit 2
    fi
    prefix=$1
    ;;
  *)
    echo "Usage: ./install.sh [PREFIX]" >&2
    exit 2
    ;;
esac

case "$prefix" in
  /*) ;;
  *)
    echo "The install prefix must be an absolute path: $prefix" >&2
    exit 2
    ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source_binary="$script_dir/taiwalog"
installed_binary="$prefix/bin/taiwalog"
staged_binary=

cleanup() {
  if [ -n "$staged_binary" ]; then
    rm -f -- "$staged_binary"
  fi
}
trap cleanup EXIT HUP INT TERM

if [ ! -x "$source_binary" ] || ! "$source_binary" --version >/dev/null; then
  echo "The release bundle does not contain a runnable taiwalog binary." >&2
  exit 1
fi

install -d "$prefix/bin"
if [ -L "$installed_binary" ]; then
  echo "Refusing to replace a symlinked Taiwalog installation: $installed_binary" >&2
  exit 1
fi
if [ -e "$installed_binary" ] && [ ! -f "$installed_binary" ]; then
  echo "Refusing to replace a non-file Taiwalog installation: $installed_binary" >&2
  exit 1
fi

staged_binary=$(mktemp "$prefix/bin/.taiwalog-install.XXXXXX")
install -m 0755 "$source_binary" "$staged_binary"
"$staged_binary" --version >/dev/null
mv -f -- "$staged_binary" "$installed_binary"
staged_binary=

"$installed_binary" --version
printf 'Installed Taiwalog at %s\n' "$installed_binary"
