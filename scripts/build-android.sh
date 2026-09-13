#!/data/data/com.termux/files/usr/bin/bash
# mterm build+deploy Android.
#
#   ./scripts/build-android.sh               # build lokal → install ke device
#   ./scripts/build-android.sh --ci          # paksa CI (workflow_dispatch) → install
#   ./scripts/build-android.sh --pull        # ambil artifact CI terakhir → install (tanpa build)
#   ./scripts/build-android.sh --pull-release  # ambil APK release SIGNED dari CI → install
#   ./scripts/build-android.sh --install     # install APK yang sudah ada (mis. hasil download)
#
# Semua jalur build/CI berakhir di: app/app/build/outputs/apk/debug/app-debug.apk
# Manual install selalu lewat scripts/adb-wireless.sh install (uninstall dulu,
# karena signature APK CI beda tiap build → install -r selalu bentrok).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ABI="arm64-v8a"
TARGET="aarch64-linux-android"
ANDROID_API=${ANDROID_API:-35}
mode="${1:-local}"

apk_local() { echo "$ROOT/app/app/build/outputs/apk/debug/app-debug.apk"; }
repo="$(cd "$ROOT" && git remote get-url origin 2>/dev/null | sed -E 's#https://github.com/##; s#git@github.com:##; s#\.git$##')"

require_apk() {
  local apk="$1"
  [ -f "$apk" ] || { echo "APK tidak ditemukan: $apk" >&2; exit 1; }
}

adb_install() {
  "$ROOT/scripts/adb-wireless.sh" install "$1"
}

build_local() {
  command -v cargo >/dev/null 2>&1 || { echo "butuh cargo" >&2; exit 1; }
  command -v cargo-ndk >/dev/null 2>&1 || cargo install cargo-ndk

  echo "▶ cargo ndk build ($ABI)"
  cargo ndk \
      -t "$TARGET" \
      -o "$ROOT/app/app/src/main/jniLibs" \
      build -p mterm-jni --release

  echo "▶ gradle assembleDebug"
  (cd "$ROOT/app" && chmod +x ./gradlew && ./gradlew assembleDebug --no-daemon)
}

ci_fetch() {
  # --pull: debug APK; --pull-release: release signed APK (Fase 8)
  local variant="${1:-debug}"
  local branch="$(cd "$ROOT" && git rev-parse --abbrev-ref HEAD)"
  local outdir="$ROOT/app/app/build/outputs/apk/debug"
  local art="mterm-debug-apk"
  local apkname="app-debug.apk"
  if [ "$variant" = "release" ]; then
    outdir="$ROOT/app/app/build/outputs/apk/release"
    art="mterm-release-apk"
    apkname="app-release.apk"
  fi
  mkdir -p "$outdir"
  echo "▶ cari artifact CI terbaru ($branch / $art)" >&2
  local run_id
  run_id="$(gh run list -R "$repo" --branch "$branch" --limit 5 \
            --json databaseId,status,conclusion,workflowName \
            --jq '.[] | select(.workflowName=="ci" and .status=="completed" and .conclusion=="success") | .databaseId' \
            | head -1)"
  [ -n "$run_id" ] || { echo "tidak ada run CI success untuk $branch" >&2; exit 1; }
  echo "  run #$run_id" >&2
  local tmpdir; tmpdir="$(mktemp -d)"
  gh run download -R "$repo" "$run_id" -n "$art" -D "$tmpdir" >/dev/null || {
    echo "artifact $art tidak ada di run #$run_id (release APK butuh secret ANDROID_KEYSTORE_*)" >&2
    exit 1
  }
  local apk; apk="$(find "$tmpdir" -name "$apkname" | head -1)"
  [ -n "$apk" ] || { echo "artifact $apkname tidak ada di run #$run_id" >&2; exit 1; }
  mkdir -p "$outdir"
  cp -f "$apk" "$outdir/$apkname"
  echo "$outdir/$apkname"
}

ci_wait() {
  local branch="$(cd "$ROOT" && git rev-parse --abbrev-ref HEAD)"
  echo "▶ dispatch workflow ci.yml ($branch)"
  gh workflow run -R "$repo" ci.yml --ref "$branch"
  local run_id=""
  for _ in $(seq 1 60); do
    run_id="$(gh run list -R "$repo" --branch "$branch" --limit 3 \
              --json databaseId,status,event,workflowName \
              --jq '.[] | select(.workflowName=="ci" and .event=="workflow_dispatch") | .databaseId' \
              | head -1)"
    [ -n "$run_id" ] && break
    sleep 5
  done
  [ -n "$run_id" ] || { echo "run workflow_dispatch tidak ketemu" >&2; exit 1; }
  echo "▶ tunggu run #$run_id"
  gh run watch -R "$repo" "$run_id" --exit-status >/dev/null
}

case "$mode" in
  --local|local)
    build_local
    adb_install "$(apk_local)"
    ;;
  --ci)
    [[ "$(cd "$ROOT" && git status --porcelain)" ]] && {
      echo "⚠ ada perubahan uncommitted — CI bangun state TERCOMMIT, bukan working tree" >&2
    }
    ci_wait
    apk="$(ci_fetch)"
    adb_install "$apk"
    ;;
  --pull|pull)
    apk="$(ci_fetch debug)"
    adb_install "$apk"
    ;;
  --pull-release|pull-release)
    apk="$(ci_fetch release)"
    adb_install "$apk"
    ;;
  --install)
    require_apk "${2:-}"
    adb_install "$2"
    ;;
  *)
    echo "mode tidak dikenal: $mode" >&2
    exit 1
    ;;
esac