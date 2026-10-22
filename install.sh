#!/bin/sh
# Build haunt and install the binary to ~/.cargo/bin, so `haunt` runs from
# anywhere. Run it again after changing the source to install the new build.
set -e

dir=$(cd "$(dirname "$0")" && pwd)

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found; install Rust first: https://rustup.rs" >&2
  exit 1
fi

# --locked: use the exact dependency versions from Cargo.lock.
# --force: reinstall even if cargo thinks the same version is installed.
cargo install --path "$dir" --locked --force

bin="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin"
case ":$PATH:" in
  *":$bin:"*) echo "Installed. Run: haunt" ;;
  *) echo "Installed to $bin, but that folder isn't on your PATH." >&2
     echo "Add this to ~/.zshrc:  export PATH=\"$bin:\$PATH\"" >&2 ;;
esac
