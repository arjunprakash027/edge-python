#!/usr/bin/env bash
# Remove the edge binary, its PATH entry, and optionally the Chromium install.sh added.

set -e

INSTALL_DIR="${EDGE_INSTALL_DIR:-$HOME/.local/bin}"

# Pick sudo only when not root and sudo exists; matches install.sh.
SUDO=""
if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
fi

# Remove Chromium via the host's native package manager. Reads /etc/os-release on Linux.
uninstall_browser() {
  case "$(uname -s)" in
    Darwin)
      if command -v brew >/dev/null 2>&1; then
        brew uninstall --cask chromium
        return
      fi
      echo "Homebrew not found; remove Chromium manually" >&2
      return 1
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
          $SUDO apt-get remove -y chromium && $SUDO apt-get autoremove -y ;;
        *" fedora "*|*" rhel "*|*" centos "*)
          $SUDO dnf remove -y chromium ;;
        *" arch "*)
          $SUDO pacman -Rs --noconfirm chromium ;;
        *" opensuse "*|*" suse "*)
          $SUDO zypper remove -y chromium ;;
        *" alpine "*)
          $SUDO apk del chromium ;;
        *)
          echo "unsupported distro (${id:-unknown}); remove Chromium manually" >&2
          return 1 ;;
      esac
      ;;
  esac
}

# 1. Binary.
if [ -f "$INSTALL_DIR/edge" ]; then
  rm -f "$INSTALL_DIR/edge"
  echo "removed $INSTALL_DIR/edge"
else
  echo "no edge binary at $INSTALL_DIR/edge"
fi

# 2. PATH entry. Leave a .edgebak in case the user wants to roll it back.
for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
  if [ -f "$rc" ] && grep -qs "$INSTALL_DIR" "$rc"; then
    sed -i.edgebak "\|export PATH=\"$INSTALL_DIR:\$PATH\"|d" "$rc"
    echo "cleaned PATH entry from $rc (backup at $rc.edgebak)"
  fi
done

# 3. Chromium. Opt-in because the user may rely on it for other apps. `edge uninstall` sets EDGE_UNINSTALL_REMOVE_BROWSER after asking in Rust, so we skip the prompt then.
case "${EDGE_UNINSTALL_REMOVE_BROWSER:-}" in
  1) uninstall_browser ;;
  0) echo "leaving Chromium installed" ;;
  *)
    if [ -t 0 ]; then
      printf "remove system Chromium too? [y/N] "
      read -r ans
      case "$ans" in
        [yY]|[yY][eE][sS]) uninstall_browser ;;
        *) echo "leaving Chromium installed" ;;
      esac
    else
      echo "leaving Chromium installed (non-interactive)"
    fi
    ;;
esac

echo "edge removed."
