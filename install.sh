#!/usr/bin/env sh
set -eu

REPO="${OPENMI_REPO:-LOWENDLAGANI/Openmi-cli}"
VERSION="${OPENMI_VERSION:-latest}"

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  # Termux / Android detection
  if [ -n "${TERMUX_VERSION:-}" ] || [ "$(uname -o 2>/dev/null || true)" = "Android" ]; then
    case "$arch" in
      aarch64|arm64) echo "aarch64-linux-android"; return ;;
      *) echo "Unsupported Android architecture: $arch" >&2; exit 1 ;;
    esac
  fi

  case "$os" in
    Linux)
      case "$arch" in
        x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) echo "Unsupported Linux architecture: $arch" >&2; exit 1 ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        arm64|aarch64) echo "aarch64-apple-darwin" ;;
        x86_64) echo "Unsupported macOS architecture: x86_64 (Intel Mac is not supported; Apple Silicon arm64 only)" >&2; exit 1 ;;
        *) echo "Unsupported macOS architecture: $arch" >&2; exit 1 ;;
      esac
      ;;
    *)
      echo "Unsupported OS: $os (use install.ps1 on Windows)" >&2
      exit 1
      ;;
  esac
}

TARGET="$(detect_target)"
ASSET="openmi-${TARGET}.tar.gz"

if [ "$VERSION" = "latest" ]; then
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
else
  BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
fi

if [ -n "${OPENMI_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$OPENMI_INSTALL_DIR"
elif [ -n "${PREFIX:-}" ] && [ -d "${PREFIX}/bin" ]; then
  # Termux default bin directory
  INSTALL_DIR="${PREFIX}/bin"
else
  INSTALL_DIR="${HOME}/.local/bin"
fi

TMP_BASE="${TMPDIR:-/tmp}"
WORK_DIR="$(mktemp -d "${TMP_BASE%/}/openmi-install.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT INT TERM

echo "Downloading ${ASSET} from ${BASE_URL}..."
curl -fsSL "${BASE_URL}/${ASSET}" -o "${WORK_DIR}/${ASSET}"

if curl -fsSL "${BASE_URL}/SHA256SUMS.txt" -o "${WORK_DIR}/SHA256SUMS.txt" 2>/dev/null; then
  EXPECTED="$(grep " ${ASSET}\$" "${WORK_DIR}/SHA256SUMS.txt" | awk '{print $1}' | head -n 1 || true)"
  if [ -n "$EXPECTED" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      ACTUAL="$(sha256sum "${WORK_DIR}/${ASSET}" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
      ACTUAL="$(shasum -a 256 "${WORK_DIR}/${ASSET}" | awk '{print $1}')"
    else
      ACTUAL=""
    fi
    if [ -n "$ACTUAL" ] && [ "$ACTUAL" != "$EXPECTED" ]; then
      echo "Checksum mismatch for ${ASSET}!" >&2
      echo "  expected: ${EXPECTED}" >&2
      echo "  actual:   ${ACTUAL}" >&2
      exit 1
    fi
    echo "Verified SHA-256 checksum."
  fi
fi

mkdir -p "$INSTALL_DIR"
tar -xzf "${WORK_DIR}/${ASSET}" -C "$WORK_DIR"
chmod +x "${WORK_DIR}/openmi"
mv "${WORK_DIR}/openmi" "${INSTALL_DIR}/openmi"

echo "Installed openmi to ${INSTALL_DIR}/openmi"
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo "Add ${INSTALL_DIR} to your PATH:"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    ;;
esac
echo "Run 'openmi login' followed by 'openmi' or 'openmi run \"hello\"' to get started."
