#!/bin/sh
# Re-run this script any time to upgrade.

set -e

BASE="${EDGE_INSTALL_BASE:-https://dylan-sutton-chavez.github.io/edge-python}"
INSTALL_DIR="${EDGE_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

target="${arch}-${os}"

case "$target" in
  x86_64-unknown-linux-musl|aarch64-unknown-linux-musl) ;;
  *) echo "no prebuilt for $target yet; build from source with 'cargo install --path cli'" >&2; exit 1 ;;
esac

mkdir -p "$INSTALL_DIR"
curl -fsSL "${BASE}/edge-${target}.tar.gz" | tar -xz -C "$INSTALL_DIR" edge
chmod +x "$INSTALL_DIR/edge"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    case "$(basename "${SHELL:-bash}")" in
      bash) rc="$HOME/.bashrc" ;;
      zsh) rc="$HOME/.zshrc" ;;
      *) rc="" ;;
    esac
    if [ -n "$rc" ] && ! grep -qs "$INSTALL_DIR" "$rc" 2>/dev/null; then
      printf '\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$rc"
      echo "added $INSTALL_DIR to PATH in $rc; run 'source $rc' or open a new shell"
    fi
    ;;
esac

"$INSTALL_DIR/edge" --version
