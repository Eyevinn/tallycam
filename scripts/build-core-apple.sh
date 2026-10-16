#!/usr/bin/env bash
# Builds the Rust core (core/ffi) for Apple platforms and generates its Swift
# bindings.
#
#   scripts/build-core-apple.sh iphoneos|iphonesimulator|macosx   one platform (Xcode pre-build)
#   scripts/build-core-apple.sh xcframework                       all iOS slices + build/core/TallyCamCore.xcframework
#
# Output (all under build/core/, ignored by git):
#   lib/<platform>/libtallycam_core_ffi.a   static library per platform
#   include/                                 TallyCamCoreFFI.h (UniFFI), tallycam_core_hot.h, module.modulemap
#   swift/TallyCamCore.swift                generated UniFFI bindings, compiled into the app
#   TallyCamCore.xcframework                (xcframework mode) for other consumers
#
# Requires rustup with the targets aarch64-apple-ios and aarch64-apple-ios-sim
# (x86_64-apple-ios too for an Intel simulator build).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/build/core"
TARGET_DIR="$ROOT/target"
LIB=libtallycam_core_ffi.a
MODE="${1:-${PLATFORM_NAME:-iphoneos}}"

# Xcode exports SDKROOT, CC, LD and friends for the iOS SDK; cargo's host
# builds (build scripts, proc macros) must not see them. Run cargo with a
# clean environment.
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
if [ -z "${DEVELOPER_DIR:-}" ]; then
    if [ -d /Applications/Xcode.app/Contents/Developer ]; then
        DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
    else
        DEVELOPER_DIR="$(xcode-select -p)"
    fi
fi
export DEVELOPER_DIR
clean_cargo() {
    env -i HOME="$HOME" PATH="$CARGO_HOME/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        CARGO_HOME="$CARGO_HOME" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
        IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}" \
        MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}" \
        CARGO_TARGET_DIR="$TARGET_DIR" DEVELOPER_DIR="$DEVELOPER_DIR" \
        cargo "$@"
}

if ! command -v "$CARGO_HOME/bin/cargo" >/dev/null 2>&1; then
    echo "error: cargo not found in $CARGO_HOME/bin. Install Rust with rustup (https://rustup.rs)." >&2
    exit 1
fi

build_target() { # triple
    # Only the static library: a cdylib for iOS is not needed.
    clean_cargo rustc --release --manifest-path "$ROOT/core/ffi/Cargo.toml" --lib --crate-type staticlib --target "$1"
}

triples_for() { # platform
    case "$1" in
        iphoneos) echo "aarch64-apple-ios" ;;
        iphonesimulator)
            # Xcode passes ARCHS; default to Apple silicon.
            local t=""
            for arch in ${ARCHS:-arm64}; do
                case "$arch" in
                    arm64) t="$t aarch64-apple-ios-sim" ;;
                    x86_64) t="$t x86_64-apple-ios" ;;
                esac
            done
            echo "$t"
            ;;
        macosx) echo "aarch64-apple-darwin" ;;
        *) echo "error: unknown platform $1" >&2; exit 1 ;;
    esac
}

build_platform() { # platform
    local platform="$1" libs=()
    for triple in $(triples_for "$platform"); do
        build_target "$triple"
        libs+=("$TARGET_DIR/$triple/release/$LIB")
    done
    mkdir -p "$OUT/lib/$platform"
    if [ "${#libs[@]}" -gt 1 ]; then
        lipo -create "${libs[@]}" -output "$OUT/lib/$platform/$LIB.tmp"
    else
        cp "${libs[0]}" "$OUT/lib/$platform/$LIB.tmp"
    fi
    # Replace only when changed, so Xcode does not relink for nothing.
    if ! cmp -s "$OUT/lib/$platform/$LIB.tmp" "$OUT/lib/$platform/$LIB"; then
        mv "$OUT/lib/$platform/$LIB.tmp" "$OUT/lib/$platform/$LIB"
    else
        rm "$OUT/lib/$platform/$LIB.tmp"
    fi
}

generate_bindings() {
    # Library mode reads the UniFFI metadata from a host build of the crate.
    clean_cargo build --release --manifest-path "$ROOT/core/ffi/Cargo.toml" --lib
    clean_cargo build --release --manifest-path "$ROOT/core/ffi/Cargo.toml" --features bindgen --bin uniffi-bindgen
    local gen="$OUT/gen"
    rm -rf "$gen"
    mkdir -p "$gen" "$OUT/include" "$OUT/swift"
    (cd "$ROOT/core/ffi" && "$TARGET_DIR/release/uniffi-bindgen" generate \
        --library "$TARGET_DIR/release/libtallycam_core_ffi.dylib" --language swift --out-dir "$gen" >/dev/null)
    copy_if_changed "$gen/TallyCamCore.swift" "$OUT/swift/TallyCamCore.swift"
    copy_if_changed "$gen/TallyCamCoreFFI.h" "$OUT/include/TallyCamCoreFFI.h"
    copy_if_changed "$ROOT/core/ffi/include/tallycam_core_hot.h" "$OUT/include/tallycam_core_hot.h"
    cat > "$gen/module.modulemap" <<'EOF'
module TallyCamCoreFFI {
    header "TallyCamCoreFFI.h"
    header "tallycam_core_hot.h"
    export *
}
EOF
    copy_if_changed "$gen/module.modulemap" "$OUT/include/module.modulemap"
}

copy_if_changed() {
    cmp -s "$1" "$2" 2>/dev/null || cp "$1" "$2"
}

case "$MODE" in
    xcframework)
        generate_bindings
        build_platform iphoneos
        ARCHS="arm64" build_platform iphonesimulator
        rm -rf "$OUT/TallyCamCore.xcframework"
        xcodebuild -create-xcframework \
            -library "$OUT/lib/iphoneos/$LIB" -headers "$OUT/include" \
            -library "$OUT/lib/iphonesimulator/$LIB" -headers "$OUT/include" \
            -output "$OUT/TallyCamCore.xcframework" >/dev/null
        echo "Built $OUT/TallyCamCore.xcframework and $OUT/swift/TallyCamCore.swift"
        ;;
    *)
        generate_bindings
        build_platform "$MODE"
        ;;
esac
