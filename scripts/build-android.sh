#!/data/data/com.termux/files/usr/bin/bash
# mterm build: compile Rust core/JNI → copy .so → gradle assembleDebug
# Jalankan dari root repo: ./scripts/build-android.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ABI="arm64-v8a"
TARGET="aarch64-linux-android"
ANDROID_API=${ANDROID_API:-35}

# 1. Pastikan deps
command -v cargo >/dev/null 2>&1 || { echo "butuh cargo" >&2; exit 1; }
command -v cargo-ndk >/dev/null 2>&1 || cargo install cargo-ndk
command -v gradlew >/dev/null 2>&1 || {
  test -x "$ROOT/app/gradlew" && GRADLE="$ROOT/app/gradlew" || {
    echo "butuh gradlew di app/ (unduh gradle wrapper)" >&2; exit 1;
  }
}
GRADLE="${GRADLE:-$ROOT/app/gradlew}"

# 2. Compile Rust cdylib langsung ke src/main/jniLibs/ (Gradle auto-pickup)
echo "▶ cargo ndk build ($ABI)"
cargo ndk \
    -t "$TARGET" \
    -o "$ROOT/app/app/src/main/jniLibs" \
    build -p mterm-jni --release

# 3. Build APK
echo "▶ gradle assembleDebug"
(cd "$ROOT/app" && chmod +x ./gradlew && ./gradlew assembleDebug --no-daemon)

echo ""
echo "✅ APK: $ROOT/app/app/build/outputs/apk/debug/mterm-debug.apk"