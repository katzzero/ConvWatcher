#!/bin/sh
set -e

chown -R convwatcher:convwatcher /app /usr/local/bin/convwatcher 2>/dev/null || true

exec su-exec convwatcher "$@"
