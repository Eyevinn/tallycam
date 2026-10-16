#!/usr/bin/env bash
# Builds the Rust core for Android (core/android: the UniFFI API of
# core/ffi plus JNI wrappers of its per-frame C ABI, one library) and
# generates its Kotlin bindings.
#
#   scripts/build-core-android.sh [ABI ...]   default: arm64-v8a
#
# Output (ignored by git):
#   android/app/src/main/jniLibs/<abi>/libtallycam_android.so
#   android/app/build/generated/uniffi/se/eyevinn/tallycam/core/*.kt
#
# Requires the Android NDK (ANDROID_NDK_HOME, or ANDROID_HOME/ndk/<version>),
# rustup targets (aarch64-linux-android, x86_64-linux-android for an x86_64
# emulator) and cargo-ndk (`cargo install cargo-ndk`). The Gradle build runs
# this script before compiling.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/android/app"
JNI_LIBS="$APP/src/main/jniLibs"
BINDINGS="$APP/build/generated/uniffi"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
ABIS=("$@")
[ ${#ABIS[@]} -eq 0 ] && ABIS=(arm64-v8a)

if [ -z "${ANDROID_NDK_HOME:-}" ]; then
    SDK="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
    NDK=$(ls -d "$SDK"/ndk/* 2>/dev/null | sort -V | tail -1 || true)
    if [ -z "$NDK" ]; then
        echo "error: no Android NDK (set ANDROID_NDK_HOME, or install one with sdkmanager)" >&2
        exit 1
    fi
    export ANDROID_NDK_HOME="$NDK"
fi

args=()
for abi in "${ABIS[@]}"; do args+=(-t "$abi"); done
# API 29 (Android 10): the app's minSdk. No debug info in the library (the
# workspace keeps line tables for the Apple builds): it would be most of the
# APK. Release APKs are stripped by the Android build anyway.
CARGO_TARGET_DIR="$TARGET_DIR" CARGO_PROFILE_RELEASE_DEBUG=0 cargo ndk --platform 29 "${args[@]}" -o "$JNI_LIBS" \
    build --release --manifest-path "$ROOT/core/android/Cargo.toml"

# cargo-ndk copies every cdylib it built; the app loads only this one.
rm -f "$JNI_LIBS"/*/libtallycam_core_ffi.so

# The bindings come from the library's UniFFI metadata (library mode); any
# ABI's library carries the same.
LIB="$JNI_LIBS/${ABIS[0]}/libtallycam_android.so"
rm -rf "$BINDINGS"
CARGO_TARGET_DIR="$TARGET_DIR" cargo run --quiet --manifest-path "$ROOT/core/ffi/Cargo.toml" \
    --features bindgen --bin uniffi-bindgen -- generate --library "$LIB" --language kotlin \
    --config "$ROOT/core/android/uniffi.toml" --no-format --out-dir "$BINDINGS"
# Two names in the core's API clash in Kotlin (UniFFI 0.32): SrtCaller.close
# with AutoCloseable.close, and error fields named `message` with
# Throwable.message. Renamed here, on the Kotlin side only, rather than in
# core/ffi, whose Swift API uses them: close -> shutdown, message -> reason.
KT=$(find "$BINDINGS" -name '*.kt')
sed -i.bak -e 's/fun `close`()/fun `shutdown`()/' \
    -e 's/`message`/`reason`/g' \
    -e 's/get() = "message=${ `reason` }"/get() = `reason`/' "$KT"
rm -f "$KT.bak"
echo "Built ${ABIS[*]} and Kotlin bindings in $BINDINGS"
