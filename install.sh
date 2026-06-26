#!/usr/bin/env sh
# smolsonic installer
#
#   curl -fsSL https://raw.githubusercontent.com/tsirysndr/smolsonic/main/install.sh | sh
#
# Environment overrides:
#   SMOLSONIC_VERSION   Release tag to install (default: latest)
#   SMOLSONIC_INSTALL   Install directory   (default: /usr/local/bin, fallback ~/.local/bin)

set -eu

REPO="tsirysndr/smolsonic"
BIN="smolsonic"
VERSION="${SMOLSONIC_VERSION:-latest}"
INSTALL_DIR="${SMOLSONIC_INSTALL:-}"

info()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
error() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || error "missing required command: $1"
}

need uname
need tar
need mktemp

if command -v curl >/dev/null 2>&1; then
  FETCH="curl -fsSL"
elif command -v wget >/dev/null 2>&1; then
  FETCH="wget -qO-"
else
  error "need curl or wget to download release assets"
fi

OS="$(uname -s)"
case "$OS" in
  Linux)   OS_TAG="linux" ;;
  Darwin)  OS_TAG="macos" ;;
  *)       error "unsupported OS: $OS (smolsonic releases cover linux and macos)" ;;
esac

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)             ARCH_TAG="amd64" ;;
  arm64|aarch64)            ARCH_TAG="aarch64" ;;
  armv7l|armv6l|arm)
    if [ "$OS_TAG" = "linux" ]; then
      ARCH_TAG="armhf"
    else
      error "unsupported architecture on $OS_TAG: $ARCH"
    fi
    ;;
  *) error "unsupported architecture: $ARCH" ;;
esac

if [ "$VERSION" = "latest" ]; then
  info "Resolving latest release for $REPO"
  VERSION="$($FETCH "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' \
    | head -n1 \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$VERSION" ] || error "could not resolve latest release tag"
fi

case "$VERSION" in
  v*) ;;
  *)  VERSION="v$VERSION" ;;
esac

ASSET="${BIN}-${VERSION}-${OS_TAG}-${ARCH_TAG}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"

if [ -z "$INSTALL_DIR" ]; then
  if [ -w "/usr/local/bin" ] 2>/dev/null; then
    INSTALL_DIR="/usr/local/bin"
  elif [ "$(id -u)" = "0" ]; then
    INSTALL_DIR="/usr/local/bin"
  else
    INSTALL_DIR="$HOME/.local/bin"
  fi
fi

mkdir -p "$INSTALL_DIR"

TMP="$(mktemp -d 2>/dev/null || mktemp -d -t smolsonic)"
trap 'rm -rf "$TMP"' EXIT INT TERM

info "Downloading $ASSET"
case "$FETCH" in
  curl*) curl -fsSL "$URL"   -o "$TMP/$ASSET"      || error "download failed: $URL" ;;
  wget*) wget -qO  "$TMP/$ASSET" "$URL"            || error "download failed: $URL" ;;
esac

if command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1; then
  info "Verifying checksum"
  if eval "$FETCH \"$URL.sha256\"" > "$TMP/$ASSET.sha256" 2>/dev/null && [ -s "$TMP/$ASSET.sha256" ]; then
    EXPECTED="$(awk '{print $1}' "$TMP/$ASSET.sha256")"
    if command -v sha256sum >/dev/null 2>&1; then
      ACTUAL="$(sha256sum "$TMP/$ASSET" | awk '{print $1}')"
    else
      ACTUAL="$(shasum -a 256 "$TMP/$ASSET" | awk '{print $1}')"
    fi
    [ "$EXPECTED" = "$ACTUAL" ] || error "checksum mismatch (expected $EXPECTED, got $ACTUAL)"
  else
    warn "no published checksum, skipping verification"
  fi
fi

info "Extracting"
tar -xzf "$TMP/$ASSET" -C "$TMP"

[ -f "$TMP/$BIN" ] || error "binary '$BIN' not found in archive"

INSTALLED="$INSTALL_DIR/$BIN"
if [ -w "$INSTALL_DIR" ]; then
  install -m 0755 "$TMP/$BIN" "$INSTALLED"
else
  info "Elevating with sudo to install into $INSTALL_DIR"
  sudo install -m 0755 "$TMP/$BIN" "$INSTALLED"
fi

info "Installed $BIN $VERSION to $INSTALLED"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    warn "$INSTALL_DIR is not on your PATH"
    warn "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac

"$INSTALLED" --version 2>/dev/null || true
