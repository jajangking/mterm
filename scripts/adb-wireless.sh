#!/data/data/com.termux/files/usr/bin/bash
# mterm dev tool: ADB wireless (Android 11+ pairing + connect)
# Pakai saat iterate APK mterm tanpa kabel.
#
# Alamat device disimpan di .mterm.env (repo root) — isi:
#   MTERM_ADB_DEVICE=192.168.1.3:35013
#   MTERM_ADB_PAIR=192.168.1.3:37000     # port pairing (opsional)
# `save` menulis file ini; `connect`/`pair` baca dari sini kalau argumen
# tidak diberikan.

set -euo pipefail

adb_bin="${ADB_BIN:-adb}"

# env file: override via MTERM_ENV, default ./.mterm.env lalu ~/.mterm.env
mterm_env="${MTERM_ENV:-}"
if [ -z "$mterm_env" ]; then
  if [ -f ./.mterm.env ]; then
    mterm_env=./.mterm.env
  elif [ -f "$HOME/.mterm.env" ]; then
    mterm_env="$HOME/.mterm.env"
  fi
fi
load_env() {
  [ -n "$mterm_env" ] && [ -f "$mterm_env" ] && . "$mterm_env" || true
}
load_env

usage() {
  cat <<EOF
mterm adb-wireless helper

  $0 save <ip:port> [pair_port]     # simpan alamat ke .mterm.env
  $0 show                           # tampilkan device tersimpan + status
  $0 pair [code]                    # pairing (pakai MTERM_ADB_PAIR tersimpan)
  $0 connect [ip:port]              # connect (default: MTERM_ADB_DEVICE)
  $0 status                         # daftar device (adb devices)
  $0 install <apk_path>             # install APK mterm (uninstall dulu)
  $0 logcat [filter]                # logcat dengan filter mterm

Contoh alur:
  1. HP: Settings > Developer options > Wireless debugging > ON
  2. $0 save 192.168.1.5:37000 45000   # (pair_port dari layar pairing)
  3. $0 pair 123456
  4. $0 install app/build/outputs/apk/debug/mterm-debug.apk
EOF
}

require_adb() {
  if ! command -v "$adb_bin" >/dev/null 2>&1; then
    echo "adb tidak ditemukan. Install: pkg install android-tools" >&2
    exit 1
  fi
}

env_target() {
  if [ -n "$mterm_env" ]; then
    echo "$mterm_env"
  else
    echo "./.mterm.env"
  fi
}

case "${1:-}" in
  save)
    require_adb
    [ $# -ge 2 ] || { usage; exit 1; }
    dev="$2"; pair="${3:-}"
    target="$(env_target)"
    {
      echo "# ditulis oleh $0"
      echo "MTERM_ADB_DEVICE=$dev"
      if [ -n "$pair" ]; then
        ip="${dev%:*}"
        echo "MTERM_ADB_PAIR=${ip}:${pair}"
      fi
    } > "$target"
    echo "disimpan di $target: $dev"
    ;;
  show)
    echo "env file: ${mterm_env:-(belum ada)}"
    echo "device   : ${MTERM_ADB_DEVICE:-(kosong)}"
    echo "pair     : ${MTERM_ADB_PAIR:-}"
    require_adb
    "$adb_bin" devices -l
    ;;
  pair)
    require_adb
    load_env
    if [ $# -eq 3 ]; then
      # pair <ip:pair_port> <code>
      peer="$2"; code="$3"
    elif [ $# -eq 2 ]; then
      # pair <code> → pakai MTERM_ADB_PAIR tersimpan
      peer="${MTERM_ADB_PAIR:-}"
      [ -n "$peer" ] || { echo "simpan dulu: $0 save <ip:port> <pair_port>" >&2; exit 1; }
      code="$2"
    else
      usage
      exit 1
    fi
    "$adb_bin" pair "$peer" "$code"
    ;;
  connect)
    require_adb
    load_env
    if [ $# -ge 2 ]; then
      dev="$2"
    else
      dev="${MTERM_ADB_DEVICE:-}"
      [ -n "$dev" ] || { echo "simpan dulu: $0 save <ip:port>" >&2; exit 1; }
    fi
    "$adb_bin" connect "$dev"
    "$adb_bin" devices -l
    ;;
  status)
    require_adb
    echo "device tersimpan: ${MTERM_ADB_DEVICE:-(kosong)}"
    "$adb_bin" devices -l
    ;;
  install)
    require_adb
    [ $# -eq 2 ] || { usage; exit 1; }
    # APK CI: keystore beda tiap run → update (-r) gagal kalau signature beda;
    # tapi uninstall kadang DELETE_FAILED_INTERNAL_ERROR di ROM tertentu (Transsion).
    # Strategi: coba uninstall (abaikan gagal) → install -r; kalau masih gagal,
    # install polos (update, kalaupun signature sama). Selalu verifikasi hash.
    "$adb_bin" shell am force-stop com.mterm.app >/dev/null 2>&1 || true
    set +e
    "$adb_bin" uninstall com.mterm.app >/dev/null 2>&1
    "$adb_bin" install -r "$2" >/dev/null 2>&1; rc=$?
    [ $rc -ne 0 ] && { "$adb_bin" install "$2" >/dev/null 2>&1; rc=$?; }
    set -e
    [ $rc -eq 0 ] || { echo "install gagal (rc=$rc): $2" >&2; exit 1; }

    # verifikasi hash — jangan percaya log "Success"
    want="$(sha256sum "$2" | awk '{print $1}')"
    apkpath="$("$adb_bin" shell pm path com.mterm.app 2>/dev/null | sed 's/package://' | tr -d '\r')"
    tmpapk="${TMPDIR:-/data/data/com.termux/files/usr/tmp}/mterm-installed.apk"
    rm -f "$tmpapk"
    [ -n "$apkpath" ] && "$adb_bin" pull "$apkpath" "$tmpapk" >/dev/null 2>&1
    got="$(sha256sum "$tmpapk" 2>/dev/null | awk '{print $1}')"
    if [ "$want" = "$got" ] && [ -n "$got" ]; then
      echo "✅ installed hash cocok: ${want:0:12}"
    else
      echo "⚠ HASH BEDA! installed=${got:0:12} file=${want:0:12}" >&2
    fi
    "$adb_bin" shell am start -n com.mterm.app/.MainActivity
    ;;
  logcat)
    require_adb
    filter="${2:-"Rust mterm AndroidRuntime"}"
    "$adb_bin" logcat -v brief | grep -Ei "$filter"
    ;;
  *)
    usage
    exit 1
    ;;
esac