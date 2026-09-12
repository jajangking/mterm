#!/data/data/com.termux/files/usr/bin/bash
# mterm dev tool: ADB wireless (Android 11+ pairing + connect)
# Pakai saat iterate APK mterm tanpa kabel.

set -euo pipefail

adb_bin="${ADB_BIN:-adb}"

usage() {
  cat <<EOF
mterm adb-wireless helper

  $0 pair <ip:port> <pairing_code>   # mulai wireless debugging pairing (Android 11+)
  $0 connect <ip:port>               # connect ke device (port dari Wireless debugging)
  $0 status                          # daftar device (adb devices)
  $0 install <apk_path>              # install APK mterm ke device yang terhubung
  $0 logcat [filter]                 # logcat dengan filter mterm (default: Rust/AndroidRuntime)

Contoh alur:
  1. HP: Settings > Developer options > Wireless debugging > ON > "Pair device..."
  2. $0 pair 192.168.1.5:37000 123456
  3. $0 connect 192.168.1.5:37777
  4. $0 install app/build/outputs/apk/debug/mterm-debug.apk
EOF
}

require_adb() {
  if ! command -v "$adb_bin" >/dev/null 2>&1; then
    echo "adb tidak ditemukan. Install: pkg install android-tools" >&2
    exit 1
  fi
}

case "${1:-}" in
  pair)
    require_adb
    [ $# -eq 3 ] || { usage; exit 1; }
    "$adb_bin" pair "$2" "$3"
    ;;
  connect)
    require_adb
    [ $# -eq 2 ] || { usage; exit 1; }
    "$adb_bin" connect "$2"
    "$adb_bin" devices -l
    ;;
  status)
    require_adb
    "$adb_bin" devices -l
    ;;
  install)
    require_adb
    [ $# -eq 2 ] || { usage; exit 1; }
    # CI debug keystore beda tiap run → update -r selalu bentrok
    # soal signature. Uninstall dulu biar install baru selalu sukses.
    "$adb_bin" uninstall com.mterm.app >/dev/null 2>&1
    "$adb_bin" install "$2"
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