#!/usr/bin/env bash
# Re-run this script any time to upgrade.

set -e

BASE="${EDGE_INSTALL_BASE:-https://cdn.edgepython.com/cli}"
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
  x86_64-unknown-linux-musl|aarch64-unknown-linux-musl|x86_64-apple-darwin|aarch64-apple-darwin) ;;
  *) echo "no prebuilt for $target yet; build from source with 'cargo install --path cli'" >&2; exit 1 ;;
esac

# Pick sudo only when not root and sudo exists; respects rootless containers.
SUDO=""
if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
fi

# True if any Chrome/Chromium-flavored binary the engine accepts is already on PATH.
have_browser() {
  command -v chromium >/dev/null 2>&1 \
    || command -v chromium-browser >/dev/null 2>&1 \
    || command -v google-chrome >/dev/null 2>&1 \
    || command -v microsoft-edge >/dev/null 2>&1
}

# Install Chromium using the host's native package manager. Reads /etc/os-release on Linux.
install_browser() {
  echo "no Chromium-flavored browser found; installing..."

  case "$(uname -s)" in
    Darwin)
      if command -v brew >/dev/null 2>&1; then
        brew install --cask chromium
        return
      fi
      echo "Homebrew not found; install Chrome/Chromium manually or set EDGE_CHROME_PATH" >&2
      exit 1
      ;;
    Linux)
      local id="" id_like=""
      if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        id="${ID:-}"
        id_like="${ID_LIKE:-}"
      fi
      case " ${id} ${id_like} " in
        *" debian "*|*" ubuntu "*)
          $SUDO apt-get update && $SUDO apt-get install -y chromium ;;
        *" fedora "*|*" rhel "*|*" centos "*)
          $SUDO dnf install -y chromium ;;
        *" arch "*)
          $SUDO pacman -Sy --noconfirm chromium ;;
        *" opensuse "*|*" suse "*)
          $SUDO zypper install -y chromium ;;
        *" alpine "*)
          $SUDO apk add --no-cache chromium ;;
        *)
          echo "unsupported distro (${id:-unknown}); install Chrome/Chromium manually or set EDGE_CHROME_PATH" >&2
          exit 1 ;;
      esac
      ;;
  esac
}

mkdir -p "$INSTALL_DIR"
curl -fsSL "${BASE}/edge-${target}.tar.gz" | tar -xz -C "$INSTALL_DIR" edge
chmod +x "$INSTALL_DIR/edge"

if ! have_browser; then
  install_browser
fi

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
