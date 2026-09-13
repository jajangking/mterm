#!/usr/bin/env bash
# mterm installer — pasang biner Linux dari tarball (lokal atau URL).
# Pakai: scripts/install.sh [path/URL-tarball.tar.gz]
set -euo pipefail

src="${1:-}"
if [ -z "$src" ]; then
    echo "error: berikan path/URL tarball, mis. scripts/install.sh mterm-linux-x86_64.tar.gz" >&2
    exit 2
fi

dst="${MTERM_INSTALL_DIR:-$HOME/.local/bin}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

case "$src" in
    http://*|https://*)
        if ! command -v curl >/dev/null; then
            echo "error: butuh curl untuk unduh tarball remote" >&2
            exit 2
        fi
        curl -fsSL -o "$tmp/tarball.gz" "$src"
        ;;
    *)
        [ -f "$src" ] || { echo "error: tarball tidak ketemu: $src" >&2; exit 2; }
        cp "$src" "$tmp/tarball.gz"
        ;;
esac

mkdir -p "$dst"
tar -xzf "$tmp/tarball.gz" -C "$tmp"
bins=( "$tmp"/bin/mterm "$tmp"/mterm-bin/mterm "$tmp"/mterm )
for b in "${bins[@]}"; do
    if [ -f "$b" ]; then
        install -m755 "$b" "$dst/mterm"
        echo "mterm terpasang: $dst/mterm"
        echo "  (jika '$dst' belum di PATH, tambahkan ke shell rc)"
        "$dst/mterm" doctor
        exit 0
    fi
done
echo "error: biner mterm tidak ditemukan di dalam tarball" >&2
exit 2