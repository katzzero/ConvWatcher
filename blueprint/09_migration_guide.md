# ConvWatcher — Migration Guide

## From DOKCONV-WATCHER v1.x to ConvWatcher v2.0

This is a **breaking change** release. Config files from v1.x are NOT compatible with v2.0.

---

## What Changed

### Structural Changes

| Aspect | v1 (DOKCONV-WATCHER) | v2 (ConvWatcher) |
|--------|---------------------|-------------------|
| Package name | `dokconv-watcher` | `convwatcher` |
| Binary name | `dokconv-watcher` | `convwatcher` |
| Singular `video:` rule | Yes | Removed — use `video_rules[]` |
| `VideoWatchRule` | Separate struct | Unified into `VideoRule` with `format: Option<String>` |
| `ImagesConfig` enum | Single/Legacy variants | Removed — use `image_rules[]` directly |
| `flexible_images` bool | Yes | Removed — all `image_rules[]` are flexible |
| `image_watch_formats` | Vec<String> | Removed — each rule's `format` defines subfolder |
| New modules | — | Audio, PDF, Document, Embedded Config |
| `custom_rules[].format` | Not supported | Added — enables subfolder mode for custom commands |
| `audio_watch`, `doc_watch`, `custom_watch` | — | New booleans for subfolder mode |
| `scan_embedded_configs` | — | New global config option |
| Embedded `mainconfig.yaml` | — | New feature: auto-register watchers |

### Config Field Changes

#### Removed Fields
```yaml
# v1 — REMOVED in v2
video:                      # → use video_rules[]
  input_extensions: [...]
  output_ext: .mp4
  codec: libx264
  ...

images:                     # → use image_rules[]
  input_extensions: [...]
  output_ext: .png
  quality: 90
  ...

flexible_images: true       # → removed, image_rules always flexible
image_watch_formats: [...]  # → removed, each rule has its own format
```

#### New/Changed Fields
```yaml
# v2 — NEW or CHANGED
watch_folder: ./watch
output_folder: ./output

video_watch: true           # KEPT (was v1)
image_watch: true           # KEPT (was v1)
audio_watch: true           # NEW
doc_watch: true             # NEW
custom_watch: true          # NEW

# Unified rule lists (all plural, all work the same way)
video_rules:
  - format: h264            # NEW: format field for subfolder mode
    input_extensions: [...] # used when format is not set
    output_ext: .mp4
    codec: libx264
    ...

image_rules:
  - format: jpg             # NEW: format field
    input_extensions: [...]
    output_ext: .jpg
    quality: 90

audio_rules:                # NEW
  - format: mp3
    output_ext: .mp3
    audio_codec: libmp3lame
    audio_bitrate: "320k"

pdf_rules:                  # NEW
  - input_extensions: [.pdf]
    output_ext: .pdf
    mode: compress
    quality: ebook

document_rules:             # NEW
  - format: epub
    output_ext: .epub
    toc: true

custom_rules:               # KEPT + format field added
  - format: compress        # NEW: optional subfolder name
    input_extensions: [...]
    output_ext: ".zip"
    command: "zip -j {output} {input}"
```

---

## Migration Steps

### Step 1: Rename Project

```bash
# In Cargo.toml
sed -i 's/name = "dokconv-watcher"/name = "convwatcher"/' Cargo.toml

# In main.rs — update env!("CARGO_PKG_VERSION") reference
# In README.md and other docs

# Rename binary references in scripts and Docker files
```

### Step 2: Update Config Files

#### `config/global.yaml` — Add new fields

```yaml
# Add to existing global.yaml:
scan_embedded_configs: true
embedded_config_name: "mainconfig.yaml"
embedded_scan_interval_s: 30
embedded_scan_paths: ["/data", "."]
```

#### `config/watchers.yaml` — Migrate video rules

**Before (v1):**
```yaml
watchers:
  - watch_folder: ./watch
    output_folder: ./output
    video:
      input_extensions: [.mp4, .avi, .mkv]
      output_ext: .mp4
      codec: libx264
      quality: "crf 23"
      audio_codec: aac
      audio_bitrate: "128k"
    video_rules: []
```

**After (v2):**
```yaml
watchers:
  - watch_folder: ./watch
    output_folder: ./output
    video_rules:
      - input_extensions: [.mp4, .avi, .mkv]
        output_ext: .mp4
        codec: libx264
        quality: "crf 23"
        audio_codec: aac
        audio_bitrate: "128k"
```

**Before (v1) — video_watch + video_rules:**
```yaml
    video_watch: true
    video_rules:
      - format: h264
        output_ext: .mp4
        codec: libx264
        quality: "crf 23"
        audio_codec: aac
        audio_bitrate: "128k"
```

**After (v2) — same:**
```yaml
    video_watch: true
    video_rules:
      - format: h264
        output_ext: .mp4
        codec: libx264
        quality: "crf 23"
        audio_codec: aac
        audio_bitrate: "128k"
```

#### Migrate image rules

**Before (v1):**
```yaml
    flexible_images: false
    images:
      input_extensions: [.jpg, .png]
      output_ext: .png
      quality: 90
    image_watch_formats: [jpg, png, gif, bmp, tiff, webp]
```

**After (v2):**
```yaml
    image_rules:
      - input_extensions: [.jpg, .jpeg, .png, .gif, .bmp, .tiff, .webp]
        output_ext: .png
        quality: 90
    # If using subfolder mode:
    image_watch: true
    image_rules:
      - format: jpg
        output_ext: .jpg
        quality: 90
      - format: png
        output_ext: .png
        quality: 100
        transparent: true
```

### Step 3: Update Health Server References

In `src/health/server.rs`, update `watcher_info_from_config()`:
- Remove `images_config` / `ImagesConfig` handling
- Add audio_rules, pdf_rules, document_rules info
- Add audio_watch, doc_watch, custom_watch flags to WatcherInfo

### Step 4: Update Processor References

In `src/main.rs` `process_jobs()`:
- Remove legacy `Video` variant (it was unified)
- Add match arms for `Audio`, `Pdf`, `Document`

In `src/watcher/monitor.rs` `create_job()`:
- Remove `image_watch`-gated image subfolder logic (use universal format matching)
- Remove `VideoRule` (singular) matching
- Add audio, pdf, document, custom format matching

### Step 5: Remove Deprecated Code

- Remove `ImagesConfig` enum
- Remove `VideoWatchRule` struct (moved to `VideoRule` with `format`)
- Remove `flexible_images` field
- Remove `image_watch_formats` field
- Remove `get_image_watch_formats()` / `get_video_watch_formats()` helpers
- Remove `VideoRule` (singular, without format) struct

### Step 6: Add New Modules

- Create `src/config/embedded.rs`
- Create `src/processor/audio.rs`
- Create `src/processor/pdf.rs`
- Create `src/processor/document.rs`
- Create `src/watcher/embedded.rs`
- Register all in their respective `mod.rs` files

---

## Quick Migration Script (Pseudocode)

```bash
#!/bin/bash
# Migrate project name and basic references

# Rename project
sed -i 's/dokconv-watcher/convwatcher/g' Cargo.toml
sed -i 's/DOKCONV-WATCHER/ConvWatcher/g' src/main.rs
sed -i 's/dokconv-watcher/convwatcher/g' Dockerfile docker-compose.yml

# Rename binary references
sed -i 's/dokconv-watcher/convwatcher/g' scripts/*.sh scripts/*.ps1

# Note: Config files must be manually migrated
echo "Config files must be manually updated. See blueprint/09_migration_guide.md"
```

---

## Testing the Migration

1. **Build**: `cargo build`
2. **Test config loading**: Run `convwatcher --config examples/watcher_sample.yaml` briefly to verify config parsing
3. **Test video**: Drop an `.mp4` file into a watch folder with video rules
4. **Test image**: Drop a `.jpg` into a watch folder with image rules
5. **Test audio**: Drop a `.wav` into a watch folder with audio rules
6. **Test PDF**: Drop a `.pdf` into a watch folder with pdf_rules
7. **Test document**: Drop a `.md` into a watch folder with document_rules
8. **Test subfolder mode**: Create `->h264/` subfolder, drop any video file in it
9. **Test embedded config**: Create a folder with `mainconfig.yaml`, watch it get auto-registered
10. **Test health dashboard**: Visit `http://localhost:8080/dashboard`
