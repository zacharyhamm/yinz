#!/bin/sh
# Install a downloaded release without requiring Rust or administrator access.
set -eu
fail() { echo "install: $*" >&2; exit 1; }
usage() { echo 'Usage: install.sh [--bin-dir DIRECTORY] [--data-dir DIRECTORY]'; }
bin_dir=${HOME:?HOME must be set}/.local/bin
case $(uname -s) in
    Linux)
        case ${XDG_DATA_HOME:-} in
            /*) data_dir=$XDG_DATA_HOME/yinz ;;
            *) data_dir=$HOME/.local/share/yinz ;;
        esac ;;
    Darwin) data_dir="$HOME/Library/Application Support/org.Yinz-Contributors.Yinz" ;;
    *) fail 'unsupported operating system' ;;
esac
while [ "$#" -gt 0 ]; do
    case $1 in
        --bin-dir|--data-dir)
            [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$1 requires a directory"
            case $1 in --bin-dir) bin_dir=$2 ;; --data-dir) data_dir=$2 ;; esac
            shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; fail "unknown argument: $1" ;;
    esac
done
# Prefix relative paths so utilities cannot interpret them as options.
case $bin_dir in /*) ;; *) bin_dir=$PWD/$bin_dir ;; esac
case $data_dir in /*) ;; *) data_dir=$PWD/$data_dir ;; esac
source_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
[ -x "$source_dir/yinz" ] || fail 'missing release executable'
[ -d "$source_dir/plugins/builtins" ] || fail 'missing builtin plugins'
# Reject symlinked installation trees rather than following them during copies.
for path in "$data_dir" "$data_dir/plugins" "$data_dir/plugins/builtins"; do
    [ ! -L "$path" ] || fail "symlinked directory: $path"
done
mkdir -p "$bin_dir" "$data_dir/plugins/builtins"
[ -z "$(find "$data_dir/plugins/builtins" -type l -print)" ] || fail 'builtin destination contains symlinks'
cp -R "$source_dir/plugins/builtins/." "$data_dir/plugins/builtins/"
staged=$(mktemp "$bin_dir/.yinz.XXXXXX")
trap 'rm -f "$staged"' EXIT HUP INT TERM
cp "$source_dir/yinz" "$staged"
chmod 755 "$staged"
[ ! -d "$bin_dir/yinz" ] || fail 'binary destination is a directory'
mv -f "$staged" "$bin_dir/yinz"
printf 'Installed %s\nBuiltin data directory: %s\nEnsure %s is on PATH.\n' "$bin_dir/yinz" "$data_dir" "$bin_dir"
printf 'If using a custom data directory, launch with: yinz --data-dir "%s"\n' "$data_dir"
