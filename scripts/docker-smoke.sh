#!/usr/bin/env bash
# Smoke-test a ConvWatcher image end-to-end inside the container.
# Usage: scripts/docker-smoke.sh [image]
set -euo pipefail

IMAGE="${1:-convwatcher:0.9.10-local}"
NAME="cw-smoke-$$"
PORT="${SMOKE_PORT:-18080}"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/cw-smoke.XXXXXX")"
cleanup() {
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

mkdir -p "$TMP/config" "$TMP/inputs/default" "$TMP/outputs/default" "$TMP/logs"
chmod -R 777 "$TMP"

cat > "$TMP/config/video_codecs.yaml" <<'YAML'
presets:
  libx264:
    codec: libx264
    quality: crf 23
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
YAML
for f in audio_codecs.yaml image_codecs.yaml pdf_presets.yaml document_presets.yaml custom_presets.yaml; do
  printf 'presets: {}\n' > "$TMP/config/$f"
done

cat > "$TMP/config/config.yaml" <<YAML
global:
  file_check_interval: 200ms
  stable_time: 500ms
  ffmpeg_path: /usr/bin/ffmpeg
  max_concurrent: 1
  refresh_interval: 1s
  embedded_secret: ""
  embedded_scan_interval: 0
  timeout: 60

  log:
    errors_file: /app/logs/errors.log
    app_log_file: /app/logs/app.log

  healthcheck:
    http_port: 8080
    bind_address: 0.0.0.0
    auth_password: "smoke"

  disk_space:
    check_interval: 60s
    threshold: 500

  history:
    persistent: false
    file: /app/logs/history.json
    max_records: 50

watchers:
- name: smoke
  watch_folder: /app/inputs/default/
  output_folder: /app/outputs/default/
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
YAML

echo "== image user =="
IMAGE_USER="$(docker image inspect --format '{{.Config.User}}' "$IMAGE")"
echo "image user: ${IMAGE_USER:-<empty>}"
[ "${IMAGE_USER:-}" = "cw" ] || { echo "FAIL: image does not run as cw" >&2; exit 1; }

echo "== start container =="
docker run -d --name "$NAME" \
  -p "127.0.0.1:${PORT}:8080" \
  -v "$TMP/config:/app/config" \
  -v "$TMP/inputs:/app/inputs" \
  -v "$TMP/outputs:/app/outputs" \
  -v "$TMP/logs:/app/logs" \
  "$IMAGE" --config /app/config/config.yaml --no-daemon --level info >/dev/null

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:${PORT}/health" >/tmp/cw-smoke-health.json 2>/dev/null; then break; fi
  if ! docker inspect -f '{{.State.Running}}' "$NAME" 2>/dev/null | grep -q true; then
    echo "FAIL: container exited during startup" >&2
    docker logs "$NAME" >&2 || true
    exit 1
  fi
  sleep 1
done
curl -fsS "http://127.0.0.1:${PORT}/health" >/tmp/cw-smoke-health.json
grep -q '"version"' /tmp/cw-smoke-health.json || { echo "FAIL: /health missing version" >&2; cat /tmp/cw-smoke-health.json >&2; exit 1; }
echo "health: $(cat /tmp/cw-smoke-health.json)"

echo "== dashboard/auth =="
[ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/dashboard")" = "200" ] || { echo "FAIL: dashboard not public" >&2; exit 1; }
[ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/api/watchers")" = "401" ] || { echo "FAIL: API not protected" >&2; exit 1; }
[ "$(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer smoke' "http://127.0.0.1:${PORT}/api/watchers")" = "200" ] || { echo "FAIL: API rejects valid password" >&2; exit 1; }
echo "dashboard 200; API 401/200 with password: OK"

echo "== ffmpeg =="
docker exec "$NAME" /usr/bin/ffmpeg -version >/dev/null
docker exec "$NAME" /usr/bin/ffprobe -version >/dev/null
[ "$(docker exec "$NAME" id -u)" != "0" ] || { echo "FAIL: container runs as root" >&2; exit 1; }
echo "ffmpeg/ffprobe OK; non-root OK"

echo "== conversion inside container =="
docker exec "$NAME" /usr/bin/ffmpeg -f lavfi -i testsrc=duration=1:size=64x64:rate=1 -y /app/inputs/default/smoke.mp4 >/dev/null 2>&1
for _ in $(seq 1 60); do
  if find "$TMP/outputs/default" -maxdepth 1 -type f | grep -q .; then break; fi
  sleep 1
done
find "$TMP/outputs/default" -maxdepth 1 -type f | grep -q . || {
  echo "FAIL: no conversion output in image" >&2
  docker logs "$NAME" >&2 || true
  exit 1
}
echo "conversion output: $(find "$TMP/outputs/default" -maxdepth 1 -type f | head -1)"
echo "SMOKE OK: $IMAGE"
