#!/usr/bin/env bash

# Link Open Half-Life for the two non-Linux release targets from an x86-64
# Linux host. The macOS build includes the adjacent parser-worker image;
# target-native CI remains responsible for running the resulting binaries.

set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly REPOSITORY_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
readonly PROFILE=release
readonly MACOS_DEPLOYMENT_TARGET=11.0

usage() {
    cat <<'EOF'
Usage: scripts/cross-build.sh <windows|macos|all>

Environment:
  CROSS_BUILD_TARGET_DIR  Cargo output root (default: target/cross)
  OSXCROSS_ROOT           Installed osxcross root; required for macos
  WINDOWS_CROSS_CC        MinGW C compiler/linker override
  WINDOWS_CROSS_AR        MinGW archiver override
  WINDOWS_CROSS_OBJDUMP   MinGW objdump override
  MACOS_CROSS_CC          osxcross Clang wrapper override
  MACOS_CROSS_AR          osxcross archiver override
EOF
}

fail() {
    printf 'cross-build: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

require_rust_target() {
    local target="$1"
    rustup target list --installed | grep -Fxq "$target" ||
        fail "Rust target $target is not installed for the active toolchain"
}

verify_artifact() {
    local artifact="$1"
    local expected="$2"
    local description

    [[ -f "$artifact" ]] || fail "Cargo completed without producing $artifact"
    description="$(file -b "$artifact")"
    [[ "$description" =~ $expected ]] ||
        fail "unexpected output format for $artifact: $description"
    printf 'Linked %s\n' "$artifact"
    printf 'Format: %s\n' "$description"
    sha256sum "$artifact"
}

verify_windows_imports() {
    local artifact="$1"
    local objdump="${WINDOWS_CROSS_OBJDUMP:-x86_64-w64-mingw32-objdump}"
    local imports
    local library

    require_command "$objdump"
    imports="$($objdump -p "$artifact" | awk '$1 == "DLL" && $2 == "Name:" { print $3 }')"
    [[ -n "$imports" ]] || fail "no PE import table found in $artifact"
    while IFS= read -r library; do
        case "${library,,}" in
            libgcc_s_*.dll | libstdc++-6.dll | libwinpthread-1.dll)
                fail "$artifact needs an unbundled MinGW runtime DLL: $library"
                ;;
        esac
    done <<<"$imports"
    printf 'PE imports (MinGW runtime-free):\n%s\n' "$imports"
}

build_windows_x86_64() {
    local target=x86_64-pc-windows-gnu
    local compiler="${WINDOWS_CROSS_CC:-x86_64-w64-mingw32-gcc}"
    local archiver="${WINDOWS_CROSS_AR:-x86_64-w64-mingw32-ar}"
    local target_dir="$CROSS_BUILD_TARGET_DIR/windows"

    require_rust_target "$target"
    require_command "$compiler"
    require_command "$archiver"

    CARGO_TARGET_DIR="$target_dir" \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$compiler" \
    CC_x86_64_pc_windows_gnu="$compiler" \
    AR_x86_64_pc_windows_gnu="$archiver" \
        cargo build --locked --release -p ohl-app --bin open-half-life --target "$target"

    verify_artifact \
        "$target_dir/$target/$PROFILE/open-half-life.exe" \
        '^PE32\+ executable .*x86-64'
    verify_windows_imports "$target_dir/$target/$PROFILE/open-half-life.exe"
}

find_osxcross_tool() {
    local suffix="$1"
    local tool

    tool="$(find -L "$OSXCROSS_ROOT/bin" -maxdepth 1 -type f \
        -name "arm64-apple-darwin*-$suffix" -print -quit)"
    [[ -n "$tool" ]] || fail "no arm64 osxcross $suffix wrapper found under $OSXCROSS_ROOT/bin"
    printf '%s\n' "$tool"
}

build_macos_aarch64() {
    local target=aarch64-apple-darwin
    local target_dir="$CROSS_BUILD_TARGET_DIR/macos"
    local compiler
    local archiver
    local app_directory
    local built_worker
    local installed_worker

    [[ -n "${OSXCROSS_ROOT:-}" ]] || fail "OSXCROSS_ROOT is required for macos"
    [[ -d "$OSXCROSS_ROOT/bin" ]] || fail "OSXCROSS_ROOT has no bin directory: $OSXCROSS_ROOT"
    compiler="${MACOS_CROSS_CC:-$(find_osxcross_tool clang)}"
    archiver="${MACOS_CROSS_AR:-$(find_osxcross_tool ar)}"

    require_rust_target "$target"
    require_command "$compiler"
    require_command "$archiver"

    PATH="$OSXCROSS_ROOT/bin:$PATH" \
    MACOSX_DEPLOYMENT_TARGET="$MACOS_DEPLOYMENT_TARGET" \
    CARGO_TARGET_DIR="$target_dir" \
    CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$compiler" \
    CC_aarch64_apple_darwin="$compiler" \
    AR_aarch64_apple_darwin="$archiver" \
        cargo build --locked --release -p ohl-app --bin open-half-life --target "$target"

    PATH="$OSXCROSS_ROOT/bin:$PATH" \
    MACOSX_DEPLOYMENT_TARGET="$MACOS_DEPLOYMENT_TARGET" \
    CARGO_TARGET_DIR="$target_dir" \
    CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$compiler" \
    CC_aarch64_apple_darwin="$compiler" \
    AR_aarch64_apple_darwin="$archiver" \
        cargo build --manifest-path crates/ohl-parser-worker/image/Cargo.toml \
            --target "$target" --release --locked

    app_directory="$target_dir/$target/$PROFILE"
    built_worker="$app_directory/ohl-media-parser-worker"
    installed_worker="$app_directory/libexec/open-half-life/ohl-media-parser-worker"
    mkdir -p "$(dirname "$installed_worker")"
    chmod 0755 "$app_directory" "$app_directory/libexec" "$(dirname "$installed_worker")"
    install -m 0555 "$built_worker" "$installed_worker"

    verify_artifact \
        "$app_directory/open-half-life" \
        '^Mach-O 64-bit .*arm64'
    verify_artifact \
        "$installed_worker" \
        '^Mach-O 64-bit .*arm64'
}

main() {
    local requested="${1:-}"
    [[ $# -eq 1 ]] || {
        usage >&2
        exit 2
    }

    require_command cargo
    require_command rustup
    require_command file
    require_command install
    require_command sha256sum
    cd -- "$REPOSITORY_ROOT"
    CROSS_BUILD_TARGET_DIR="${CROSS_BUILD_TARGET_DIR:-$REPOSITORY_ROOT/target/cross}"
    export CROSS_BUILD_TARGET_DIR

    case "$requested" in
        windows)
            build_windows_x86_64
            ;;
        macos)
            build_macos_aarch64
            ;;
        all)
            build_windows_x86_64
            build_macos_aarch64
            ;;
        -h | --help)
            usage
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
}

main "$@"
