#!/usr/bin/env bash
# Optional WSL build adapter. Output is a native PE; the installed app never uses WSL.
set -euo pipefail
cd "$(dirname "$0")/../../.."
: "${WRITER_LLVM_ROOT:?Set WRITER_LLVM_ROOT to an LLVM installation with clang-cl and lld-link}"
: "${WRITER_WINDOWS_SDK:?Set WRITER_WINDOWS_SDK to an xwin-compatible crt/sdk directory}"
export PATH="$WRITER_LLVM_ROOT/bin:$PATH"
export LD_LIBRARY_PATH="$WRITER_LLVM_ROOT/lib:${LD_LIBRARY_PATH:-}"
export CC_x86_64_pc_windows_msvc=clang-cl
export AR_x86_64_pc_windows_msvc=llvm-lib
export CFLAGS_x86_64_pc_windows_msvc="-imsvc $WRITER_WINDOWS_SDK/crt/include -imsvc $WRITER_WINDOWS_SDK/sdk/include/ucrt -imsvc $WRITER_WINDOWS_SDK/sdk/include/um -imsvc $WRITER_WINDOWS_SDK/sdk/include/shared"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=lld-link
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-L native=$WRITER_WINDOWS_SDK/crt/lib/x86_64 -L native=$WRITER_WINDOWS_SDK/sdk/lib/ucrt/x86_64 -L native=$WRITER_WINDOWS_SDK/sdk/lib/um/x86_64 -C target-feature=+crt-static"
cargo build --locked --target x86_64-pc-windows-msvc --bin metrix-writer-api
cargo test --locked --target x86_64-pc-windows-msvc --lib --no-run
