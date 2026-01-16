#!/usr/bin/env bash
set -euo pipefail

REPO_INPUT="${PTYCTL_REPO:-nfshanq/ptyctl}"
PREFIX="${PREFIX:-/usr/local}"
BIN_DIR="${PREFIX}/bin"
BIN_NAME="ptyctl"
AGENT="codex"
TRANSPORT="stdio"
HTTP_LISTEN="127.0.0.1:8765"
AUTH_TOKEN="${PTYCTL_AUTH_TOKEN:-}"

REPO="${REPO_INPUT}"
REPO="${REPO#https://github.com/}"
REPO="${REPO%.git}"
REPO="${REPO#/}"

usage() {
  cat <<'USAGE'
Usage: install.sh [--agent codex|cursor|vscode] [--transport stdio|http]
                 [--http-listen host:port] [--auth-token TOKEN]

Defaults:
  --agent codex
  --transport stdio
  --http-listen 127.0.0.1:8765

Examples:
  # STDIO + Codex (default)
  curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash

  # STDIO + Cursor/VSCode
  curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash -s -- --agent cursor

  # HTTP + Codex
  curl -fsSL https://raw.githubusercontent.com/nfshanq/ptyctl/main/install.sh | bash -s -- --transport http
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --agent)
      AGENT="${2:-}"; shift 2 ;;
    --transport)
      TRANSPORT="${2:-}"; shift 2 ;;
    --http-listen)
      HTTP_LISTEN="${2:-}"; shift 2 ;;
    --auth-token)
      AUTH_TOKEN="${2:-}"; shift 2 ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1 ;;
  esac
done

case "${AGENT}" in
  codex) ;;
  cursor|vscode) AGENT="cursor" ;;
  *)
    echo "Unsupported agent: ${AGENT} (use codex|cursor|vscode)" >&2
    exit 1 ;;
esac

case "${TRANSPORT}" in
  stdio|http) ;;
  *)
    echo "Unsupported transport: ${TRANSPORT} (use stdio|http)" >&2
    exit 1 ;;
esac

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

if [[ -z "${AUTH_TOKEN}" ]]; then
  AUTH_TOKEN="YOUR_TOKEN"
fi

echo ""
echo "Next steps:"
if [[ "${TRANSPORT}" == "stdio" ]]; then
  echo "  - Start server: ${BIN_DIR}/${BIN_NAME} serve --transport stdio"
  if [[ "${AGENT}" == "codex" ]]; then
    echo "  - Add to Codex:"
    echo "      codex mcp add ptyctl-stdio --env PTYCTL_LOG_LEVEL=info -- ${BIN_DIR}/${BIN_NAME} serve --transport stdio"
  else
    cat <<EOF
  - Add to Cursor/VSCode (settings.json):
    {
      "mcpServers": {
        "ptyctl-stdio": {
          "command": "${BIN_DIR}/${BIN_NAME}",
          "args": ["serve", "--transport", "stdio"],
          "env": {
            "PTYCTL_LOG_LEVEL": "info"
          }
        }
      }
    }
EOF
  fi
else
  echo "  - Start server:"
  echo "      ${BIN_DIR}/${BIN_NAME} serve --transport http --http-listen ${HTTP_LISTEN} --auth-token ${AUTH_TOKEN}"
  if [[ "${AGENT}" == "codex" ]]; then
    echo "  - Add to Codex:"
    echo "      export PTYCTL_AUTH_TOKEN=${AUTH_TOKEN}"
    echo "      codex mcp add ptyctl-http --url http://${HTTP_LISTEN}/mcp --bearer-token-env-var PTYCTL_AUTH_TOKEN"
  else
    cat <<EOF
  - Add to Cursor/VSCode (settings.json):
    {
      "mcpServers": {
        "ptyctl-http": {
          "url": "http://${HTTP_LISTEN}/mcp",
          "headers": {
            "Authorization": "Bearer ${AUTH_TOKEN}"
          }
        }
      }
    }
EOF
  fi
fi
