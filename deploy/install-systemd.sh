#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "error: please run as root"
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SRC="$ROOT_DIR/target/release/arc-gateway"
SERVICE_SRC="$ROOT_DIR/deploy/arc-gateway.service"
CONFIG_SRC_JSON="$ROOT_DIR/arc.example.json"

if [[ ! -f "$BIN_SRC" ]]; then
  echo "error: binary not found at $BIN_SRC"
  echo "hint: run 'cargo build --release -p arc-gateway' first"
  exit 1
fi

if [[ ! -f "$SERVICE_SRC" ]]; then
  echo "error: service file not found at $SERVICE_SRC"
  exit 1
fi

if [[ ! -f "$CONFIG_SRC_JSON" ]]; then
  echo "error: sample config not found at $CONFIG_SRC_JSON"
  exit 1
fi

install -m 0755 "$BIN_SRC" /usr/local/bin/arc-gateway
install -d -m 0755 /etc/arc
install -m 0644 "$CONFIG_SRC_JSON" /etc/arc/arc.json
install -m 0644 "$SERVICE_SRC" /etc/systemd/system/arc-gateway.service

systemctl daemon-reload
systemctl enable arc-gateway

echo "Done. Edit /etc/arc/arc.json then: systemctl start arc-gateway"
