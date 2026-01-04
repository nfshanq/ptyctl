#!/usr/bin/env bash
set -euo pipefail

REPO_INPUT="${PTYCTL_REPO:-nfshanq/pytctl}"
PREFIX="${PREFIX:-/usr/local}"
BIN_DIR="${PREFIX}/bin"
BIN_NAME="ptyctl"

REPO="${REPO_INPUT}"
REPO="${REPO#https://github.com/}"
REPO="${REPO%.git}"
REPO="${REPO#/}"

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "${OS}-${ARCH}" in
  linux-x86_64) ASSET="ptyctl-linux-amd64.tar.gz" ;;
  darwin-arm64) ASSET="ptyctl-macos-arm64.tar.gz" ;;
  *) echo "Unsupported OS/arch: ${OS}-${ARCH}" && exit 1 ;;
esac

URL="https://github.com/${REPO}/releases/latest/download/${ASSET}"
TMP_DIR="$(mktemp -d)"
TAR_PATH="${TMP_DIR}/${ASSET}"

cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

echo "Downloading ${URL}"
curl -fsSL -o "${TAR_PATH}" "${URL}"
tar -xzf "${TAR_PATH}" -C "${TMP_DIR}"

SRC_BIN="${TMP_DIR}/${ASSET%.tar.gz}"
if [[ ! -f "${SRC_BIN}" ]]; then
  echo "Binary not found in archive: ${SRC_BIN}"
  exit 1
fi

if [[ ! -w "${BIN_DIR}" ]]; then
  sudo install -m 0755 "${SRC_BIN}" "${BIN_DIR}/${BIN_NAME}"
else
  install -m 0755 "${SRC_BIN}" "${BIN_DIR}/${BIN_NAME}"
fi

echo "Installed ${BIN_DIR}/${BIN_NAME}"
