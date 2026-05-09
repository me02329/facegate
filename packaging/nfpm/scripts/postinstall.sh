#!/usr/bin/env sh
set -eu

mkdir -p /etc/facegate
mkdir -p /usr/share/facegate/models
mkdir -p /var/lib/facegate/users

chown -R root:root /etc/facegate /usr/share/facegate /var/lib/facegate
chmod 755 /var/lib/facegate /var/lib/facegate/users
chmod 644 /etc/facegate/config.toml 2>/dev/null || true
