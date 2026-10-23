#!/usr/bin/env bash
# Hosted Ubuntu already provides CMake/Clang and libclang runtime via clang-N.
# Bindgen needs the shared library, not the large libclang development package.
set -euo pipefail
cmake --version
clang --version
locate_libclang() {
  python3 - <<'PY'
import ctypes, glob
for pattern in ('/usr/lib/llvm-*/lib/libclang.so*', '/usr/lib/x86_64-linux-gnu/libclang-*.so*'):
    for path in sorted(glob.glob(pattern), reverse=True):
        try:
            lib = ctypes.CDLL(path)
            lib.clang_getClangVersion
            print(path)
            raise SystemExit(0)
        except (OSError, AttributeError):
            pass
raise SystemExit(1)
PY
}
if ! clang_lib=$(locate_libclang); then
  sudo apt-get -o Acquire::Retries=2 -o Acquire::http::Timeout=30 update
  sudo apt-get -o Acquire::Retries=2 -o Acquire::http::Timeout=30 install -y --no-install-recommends libclang1-18
  clang_lib=$(locate_libclang)
fi
printf 'LIBCLANG_PATH=%s\n' "$clang_lib" >> "$GITHUB_ENV"
printf 'Using libclang: %s\n' "$clang_lib"
