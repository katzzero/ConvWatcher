# 02 — Config Subsystem Review

**Scope:** `src/config/{mod,global,watch,embedded,codec_registry}.rs`, `convwatcher-common/src/config.rs`, `src/cli.rs`  
~16 findings. See `00-index.md` for priorities and the `embedded_secret` scope decision.

---

## 🔴 CRITICAL

### C1 — `interval: 0` panics at startup (documented "0 disables scanning" is false)
**Files:** `src/config/mod.rs:440` (docs), `src/config/global.rs:357-362` (`check_interval_s`), `src/processor/disk.rs:122-123`, `src/main.rs:137`, `src/watcher/monitor.rs:74`.

AGENTS.md + generated config comments state `0` disables scanning for `file_check_interval`, `refresh_interval`, `embedded_scan_interval`, `check_interval`. Only `embedded_scan_interval` is actually guarded (`src/watcher/embedded.rs:205-208` early-returns when 0). The other two feed straight into `tokio::time::interval(Duration::from_secs(0))` / `Duration::from_millis(0)` → **panics** (`period must be non-zero`).
- `disk.rs:123` → panic when `check_interval_s == 0`
- `monitor.rs:74` → panic when `file_check_interval_ms == 0`

**Trigger:** Operator follows documented advice, sets `disk_space.check_interval: 0` (or `file_check_interval: 0`) to disable that scanner. Daemon panics on boot.

**Fix:** Mirror the embedded scanner's guard at all call sites **and** in `validate_config`: reject `0` or treat as "disabled" (skip spawning the task / return early) before constructing `tokio::time::interval`. Add a unit test asserting `validate_config` rejects a zero `file_check_interval`/`check_interval` if choosing the reject path.

---

## 🟠 HIGH

### H1 — A malformed `config.yaml` is destructively overwritten with defaults
**File:** `src/config/mod.rs:50-57` (and `:540`).

When `serde_yaml::from_str` fails, the code logs a `warn!`, then calls `generate_default_config(&path)`, which at line 540 does `fs::write(config_path, &default_yaml)` — the user's hand-edited config is **replaced** with the default template. Combined with H3 below (where valid-looking YAML also fails to parse), a single typo can silently destroy an entire production configuration.

**Trigger:** Any YAML syntax error or unsupported-threshold string (see H3) in `config.yaml`.

**Fix:** On parse failure, return an `Err` with the parse error and do **not** touch the file. If auto-recovery is desired, back the broken file up to `config.yaml.bak.<timestamp>` before overwriting, and emit an `error!` (not `warn!`).

### H2 — Serde defaults for `log.errors_file` and `history.file` are relative, but validation rejects relative paths
**Files:** `src/config/global.rs:307-309` (`default_errors_file() -> "./logs/errors.log"`), `:407-409` (`default_history_file() -> "./logs/history.json"`); `src/config/mod.rs:100-101` (`validate_absolute_path` on both), `:201-211`.

`LogConfig`/`HistoryConfig` use `#[serde(default = "default_errors_file")]` etc., so any user config that contains a `log:` (or `history:`) block but omits the file path gets the **relative** default `"./logs/errors.log"`. `validate_config` then calls `validate_absolute_path`, which bails because the path is not absolute. The daemon refuses to start. The generated default works only because `generate_default_config` overrides these with absolute (`logs_base.join(...)`) values — masking the inconsistency.

**Trigger:** User writes `log: { max_log_files: 50 }` (or merely `log: {}`) without `errors_file`. Startup fails with `log.errors_file: must be an absolute path`.

**Fix:** Make the serde defaults absolute (compute lazily against CWD during `load_config` the same way `generate_default_config` does); or skip the absolute-path check when the value equals the documented default. Cleanest: never ship a relative serde default.

### H3 — Documented `DiskSpaceThreshold` forms `5Gb` / `10%` cannot deserialize and crash config loading
**Files:** `src/config/global.rs:389-395` (`#[serde(untagged)] enum DiskSpaceThreshold { Mb(u64), Gb(f64), Percent(f64) }`), `src/config/mod.rs:444-447` (docs: `# Exemplos: 500 (MB), 5Gb, 10%`).

`untagged` enums try each variant in order against the YAML scalar. `5Gb` and `10%` are **strings**, so `Mb(u64)`, `Gb(f64)`, and `Percent(f64)` all fail to parse them → the whole `disk_space` block fails → `GlobalConfig` fails to deserialize → `load_config` hits the H1 destructive-overwrite path. Only bare integers (interpreted as MB) and bare floats actually work. The documented examples (`5Gb`, `10%`) are landmines.

**Trigger:** User copies the documented example `threshold: 5Gb` (or `10%`) into their config.

**Fix:** Implement a custom deserializer for `DiskSpaceThreshold` that parses strings of the form `<num>` (MB), `<num>Gb`/`gb`, `<num>%`. Then add tests covering all documented forms. Also make H1 non-destructive so even an unsupported form isn't catastrophic.

### H4 — `healthcheck.bind_address` default mismatch: code default `127.0.0.1` vs generated/documented `0.0.0.0`
**Files:** `src/config/global.rs:342-353` (`default_bind_address() -> "127.0.0.1"`, `Default` impl uses `127.0.0.1`); `src/config/mod.rs:433-434` (`bind_address: 0.0.0.0` and comment `# Default: "0.0.0.0"`).

Two divergent defaults. The generated config exposes the health dashboard (which AGENTS.md flags as a stored-XSS surface even with `escapeHtml`) to the entire network by default; a user who *omits* the field relying on the documented default silently gets `127.0.0.1`. Whichever behavior is intended, the two sources disagree.

**Trigger:** First-run generation binds `0.0.0.0`; omitting the key later flips to `127.0.0.1` with no warning.

**Fix:** Pick one → security recommends `127.0.0.1` as the actual default; require explicit `0.0.0.0`. Fix the inline comment to match. Add `deny_unknown_fields` (M4) so a typo like `bind_addres:` doesn't silently flip back.

### H5 — Standalone daemon's `ffprobe_path` fallback uses the ffmpeg binary itself, not its sibling ffprobe
**Files:** `src/config/global.rs:172-176` (`ffprobe_path: Option<String>`); generated docs `src/config/mod.rs:373-375` ("se omitido, usa o mesmo diretório do ffmpeg_path"); consumer `src/main.rs:184-187` and `src/bin/server.rs:181-184`.

The documented behavior is "if `ffprobe_path` is omitted, use the same directory as `ffmpeg_path`" (i.e. derive `/usr/bin/ffprobe` from `/usr/bin/ffmpeg`). The agent binary implements exactly that (`convwatcher-agent/src/main.rs:117-123`). But the standalone daemon does `unwrap_or_else(|| global_config.ffmpeg_path.clone())` → `ffprobe` points at the **ffmpeg** binary. `get_video_duration` (`src/processor/video.rs:252-272`) runs ffmpeg with ffprobe flags → empty stdout → `unwrap_or(0.0)` → `0.0` for every file. The `check_duration` validation then compares `0.0 / 0.0` and either NaN-compares false (every conversion flagged) or trivially passes — defeating the safety check silently. *(See 01 §H1 for full impact.)*

**Trigger:** Any standalone deployment omitting `ffprobe_path` (the documented "optional" case) with `check_duration: true`.

**Fix:** Replicate the agent logic in `main.rs` and `src/bin/server.rs`:
```rust
.unwrap_or_else(|| {
    Path::new(&global_config.ffmpeg_path).parent()
        .map(|p| p.join("ffprobe").to_string_lossy().to_string())
        .unwrap_or_else(|| "/usr/bin/ffprobe".to_string())
})
```
Better: compute inside `GlobalConfig` or a helper so both binaries share it. Probe `ffprobe -version` at startup and `warn!`/fail fast if not.

---

## 🟡 MEDIUM

### M1 — `validate_absolute_path` accepts `..` components and symlinks; no canonicalization
**File:** `src/config/mod.rs:201-211`.
The check is only `Path::new(path).is_absolute()`. Paths like `/app/outputs/../../etc/cron.d/evil` and `/app/outputs` pointing at a symlink to `/etc` pass validation; `create_directories` happily `create_dir_all`s them. The processor-layer `namer::validate_output_path` rejects `..` at *output*-path construction time, but `watch_folder` is never re-checked.
**Fix:** Canonicalize-then-verify; iterate `Path::components()` and reject any `Component::ParentDir` before canonicalization as well as paths that resolve outside an expected root.

### M2 — `config/watchs` directory is hardcoded relative to CWD while every other path must be absolute
**File:** `src/config/mod.rs:235` (`fs::create_dir_all("config/watchs")`); also `src/watcher/monitor.rs:274` and `src/main.rs:260` use the literal `"config/watchs"`.
`load_config` honors `--config /anywhere/config.yaml` (computing `config_dir` from its parent), but `create_directories` always creates `config/watchs` relative to CWD. With `--config /opt/cw/config.yaml`, presets load from `/opt/cw/` but promoted overrides land in `$CWD/config/watchs` — a silent split between where the daemon *reads* overrides and where `monitor.rs` *writes* them. *(Same root cause as 03 §H6.)*
**Fix:** Derive `watchs_dir` from `config_dir` (`config_dir.join("watchs")`); pass that absolute path to the embedded scanner and promotion code.

### M3 — `codec_presets.*` names are joined unsanitized; absolute/`..` names escape the config dir
**File:** `src/config/codec_registry.rs:14-28`, `:113-121`; docs `src/config/mod.rs:399-405`.
`PathBuf::join(absolute_path)` **replaces** the base. So `codec_presets.video: "/etc/some.yaml"` loads an arbitrary file as the video preset registry; `video: "../secret.yaml"` escapes `config_dir`. Since custom presets carry an arbitrary `command` (RCE surface), loading an attacker/errant preset file from outside `config/` is an RCE-by-misconfiguration vector.
**Fix:** Validate each `codec_presets.*` entry: reject absolute paths and any `Component::ParentDir`; after joining, canonicalize and assert the result still resides within `config_dir`. Add a test.

### M4 — `GlobalConfig` has no `deny_unknown_fields`; typos in security-critical keys silently take insecure defaults
**File:** `src/config/global.rs:156-214` (and `LogConfig`, `HealthcheckConfig`, `DiskSpaceConfig`, `HistoryConfig`, `WorkerConfig`).
Concrete harm:
- A typo `embeded_secret: "strong-secret"` → the real `embedded_secret` stays at its `#[serde(default)]` empty string → override/agent auth bypassed (the security warning in `watcher/embedded.rs:210-217` and `monitor.rs:243-248` fires, but the daemon is already exposed).
- A typo `bind_addres: "127.0.0.1"` → silently falls to `0.0.0.0` (or vice-versa per H4).
- A typo `max_concurent: 8` → silently runs with the default `4`.

Rule structs (`watch.rs:57,95,119,149,214,256`) correctly use `deny_unknown_fields`; the global config does not.

**Fix:** Add `#[serde(deny_unknown_fields)]` to `GlobalConfig` and every nested config struct. Verify no intentional "ignored" keys break first.

### M5 — `max_concurrent_conversions` and `rules` are not validated for `0`/emptiness
**File:** `src/config/mod.rs:98-199`.
`validate_config` checks only that `input_extensions` is non-empty per rule. It does not:
- Reject `max_concurrent_conversions == 0` → `main.rs:125` builds a semaphore with 0 permits → every job acquire blocks forever → silent total deadlock (cf. 05 §C1).
- Warn/error on an empty `rules` vec → watcher boots, observes files, never matches → silent no-op.

**Fix:** Reject `max_concurrent_conversions == 0` (or clamp to 1 with a `warn!`); emit a `warn!` when a watcher has zero rules.

### M6 — `subfolder` fields are unsanitized; crafted names can route outside the watch folder
**File:** `src/config/watch.rs:64` (`VideoRule.subfolder`), `:100,124,153,219,261`; no validation in `mod.rs:validate_config`.
`subfolder: Option<String>` consumed downstream to build/match `->{name}/` directories. No charset restriction → `subfolder: "../escape"`, `"/etc"`, `"a/b"` are accepted and `create_dir_all`'d.
**Trigger:** Operator misconfiguration or a promoted embedded override (which can set arbitrary rule fields via its flattened `WatchType`) using `..` or `/`.
**Fix:** In `validate_config`, assert each `subfolder` matches `^[A-Za-z0-9._-]+$` and contains no `..`/path separator; reject otherwise. Optionally cross-check that the subfolder is declared in the watcher's `subfolders` list.

### M7 — Negative durations silently saturate to `0`, then trigger the C1 panic
**Files:** `src/config/global.rs:114-116` (`visit_i64` → `v.max(0) as u64`), `:55-57,62-64,69-71` (`parse::<f64>().map(|n| (n * 1000.0) as u64)`).
`(-5.0 * 1000.0) as u64` saturates to `0` (Rust ≥ 1.45). So `stable_time: -5s`, `file_check_interval: -1s`, etc. deserialize to `0` ms — which then panics (C1) for interval-ticker fields, or yields an instant-stable timer (`stable_time == 0`) that queues files mid-copy → truncated/corrupt conversions. `visit_i64` does the same `max(0)` clamp.
**Trigger:** A negative value in any duration field.
**Fix:** Reject negatives explicitly: in `parse_duration_to_ms`/`parse_duration_to_s`, if parsed `f64` is `< 0.0`, return `None` (→ "invalid duration" error); in `visit_i64`, return `E::custom("duration must be non-negative")` for `v < 0` instead of clamping.

### M8 — `validate_config` does not validate `ffmpeg_path` / `ffprobe_path`
**File:** `src/config/mod.rs:98-199`.
Only `log.errors_file`, `history.file`, and per-watcher `watch_folder`/`output_folder` get absolute-path checks. `ffmpeg_path` (and `ffprobe_path` when present) are accepted blindly — relative or empty values fail later at runtime with a confusing ffmpeg-not-found error; H5 already shows the empty-ffprobe fallback is wrong.
**Fix:** `validate_absolute_path(&global.ffmpeg_path, "global.ffmpeg_path")?` and the same for `ffprobe_path` when `Some`. Optionally verify the binaries exist/executable at startup.

### M9 — `0` for `stable_time` is a silent footgun
**File:** `src/config/global.rs:165-170`; consumer `src/watcher/monitor.rs:99,108`.
`stable_time: 0` means "queue on the first scan where size hasn't changed since the previous scan." For files on a network share that briefly pause during copy, that falsely declares stability → conversion of a half-written file → corrupted output. AGENTS.md describes the stability state machine as deliberately requiring `stable_time` default 5s; `0` voids that protection with no warning.
**Fix:** At minimum `warn!` when `stable_time_ms == 0`; consider rejecting it for watch types with expensive conversions (video/audio).

---

## 🟢 LOW / Best-practice

| ID | File:line | One-liner |
|----|-----------|-----------|
| L1 | AGENTS.md / mod.rs:560-610 | "Preset trap" doc is wrong: `generate_preset_files` writes **only** the missing file, preserves existing ones. Update doc. |
| L2 | mod.rs:34-38 | Pointless `.clone()` of full `Vec` + six `HashMap`s — `return Ok(default);`. |
| L3 | watch.rs:166 vs codec_registry.rs:73 | `quality` vs `pdf_quality` naming inconsistency; unify. |
| L4 | mod.rs:128-195 | Six duplicated extension-non-empty match arms — factor a helper. |
| L5 | watch.rs:91 | `min_duration_ratio` not range-checked → `>1.0` rejects every conversion; NaN mishandled. |
| L6 | cli.rs:8-12 | `--daemon`/`--no-daemon` independent booleans, no mutual exclusivity → use `ArgGroup`. |
| L7 | common/config.rs:54-63 | pipe `mp4`/`mov`/`m4a`/`aac` map to `matroska` muxer → `.mp4` file with Matroska container; document or warn. |
| L8 | common/config.rs:30-39 | `temp\|file`, `pipe\|stream` silent `FromStr` aliases — add to help text. |
| L9 | config/embedded.rs:5-14 | `EmbeddedConfig` no `deny_unknown_fields`; `ouput_folder:` typo → empty default. |

---

## Security summary (cross-cutting)

**Secret comparison is implemented correctly with `ConstantTimeEq`** in both the promotion path (`src/watcher/monitor.rs:250-255`) and the embedded scanner (`src/watcher/embedded.rs:135-136`). ✓ The one caveat: `subtle::ct_eq` on unequal-length slices returns early with `Choice(0)`, leaking the secret *length* (not content) — acceptable in practice; fix only if you consider length leakage in your threat model.

**Auth-bypass-by-default (documented and intentional):** `embedded_secret == ""` causes override acceptance with only a `warn!` (`monitor.rs:243-248`, `embedded.rs:210-217`). This is by design but is the single largest security exposure: an empty `embedded_secret` — the serde default and what the generated template ships (`mod.rs:392`) — lets anyone with write access to a watch folder drop `<watcher-name>.yaml` and redirect outputs (including to `..` paths per M1/M6) or, for `custom` type, run arbitrary `command` strings. See 00-index scope note for the A/B decision.

**RCE surface via custom presets:** `resolve_custom_rule` (`codec_registry.rs:399-416`) pulls `command` from the preset or rule. Execution is **not** via a shell — `build_argv` (`external.rs:133-160`) splits on whitespace and replaces placeholders per-token, and rejects `..` in expanded tokens — so classic shell-metachar injection is avoided. ✓ Residual risks: (a) the program name token could be a relative path resolved against CWD/path lookup — consider rejecting/absolutizing the program token; (b) M3 lets a custom preset be loaded from anywhere if `codec_presets.custom` points outside `config/`.

**TOCTOU on override promotion** (`monitor.rs:225-296`): the file is read, validated, then `std::fs::copy`'d to `config/watchs`. An attacker who can mutate the file between the stability check and the copy could swap content after secret validation — but they already need write access to the watch folder (which grants override power anyway once the secret is known/empty), so this is low practical impact. A cheap hardening: re-read-and-revalidate inside the copy, or `rename` the validated file into place atomically.