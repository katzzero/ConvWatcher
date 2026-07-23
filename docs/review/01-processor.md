# 01 — Processor Subsystem Review

**Scope:** `src/processor/{runner,mod,job,disk,namer,video,audio,image,pdf,document,external}.rs`  
~11 findings. See `00-index.md` for priorities and the `embedded_secret` scope decision.

---

## 🔴 CRITICAL

### C1 — Arbitrary absolute-path file write via `output_name` template
**File:** `src/processor/namer.rs:19-26`, `validate_output_path` at `:67-89`.

`OutputNamer::generate_path` does `output_folder.join(filename)` then `validate_output_path(&path, output_folder)`. `PathBuf::join` with an **absolute** argument **replaces** the base, so a template `output_name: "/etc/owned.mp4"` yields `path = "/etc/owned.mp4"`. Both guards in `validate_output_path` are bypassable:
1. The `..`-component loop (`:69-73`) only rejects `Component::ParentDir` — an absolute path has `[RootDir, Normal("etc"), ...]`, no `ParentDir` → passes.
2. The containment check (`:76-86`) is dead code: `path.canonicalize()` fails for a not-yet-created output file, so the `if let` is false and the test is skipped. The only existing test (`test_validate_output_path_accepts_normal`, `:102`) uses a non-existent path — it passes by luck, never exercises containment.

**Trigger / impact:** Default `embedded_secret` is empty (see 00-index scope note), so an unauthenticated override YAML dropped in a watch folder is accepted. An attacker sets `output_name: "/app/config/<x>"` (or any sensitive path); the next matched conversion writes there. Docker images run as root → container-escape / arbitrary-write primitive: overwrite config, secret, presets, `/etc/...`.

**Fix:** In `validate_output_path` reject `Component::RootDir` (and `Component::Prefix` on Windows). Make containment work without requiring the file to exist: canonicalize `output_folder` (exists) and `path.parent()` (parent exists once placed inside), then assert `canonical_parent.starts_with(&canonical_folder)`. Reuse validation at config-load / override-promotion time. Add tests: absolute `output_name`, leading `/`, non-existent escaping path.

### C2 — `..` substring check in custom processor is over-broad and at the wrong layer
**File:** `src/processor/external.rs:152-154` (`build_argv`), `:162-166` (`validate_command_template`).

Guards rely on `replaced.contains("..")` / `template.contains("..")` — a naive **substring** test, not a path-component test.
- **False positives (denial of service):** A source file named `v2.0..rc1.mp4` → `basename = "v2.0..rc1"` → any token containing `{basename}` `contains("..")` → `bail!`. Legitimate files with `..` in their *name* can never be processed by a custom rule.
- **Wrong layer:** The real containment protection is `OutputNamer::generate_path` (per C1, broken). The substring check gives false assurance.

**Fix:** Drop the substring heuristic. Validate traversal at the `Path`-component level (fix C1 first), and only on the actual substituted `{output}`/`{output_folder}` paths, not on every token (a `{basename}` legitimately may contain `..`).

---

## 🟠 HIGH

### H1 — Video duration sanity check silently disabled by default (ffprobe path falls back to ffmpeg)
**Files:** `src/processor/video.rs:129-145`, `252-266`; root cause `src/main.rs:184-187` and `src/bin/server.rs:181-184`. *(Same root cause as 02 §H5.)*

`GlobalConfig.ffprobe_path` defaults to `None` (`config/global.rs:176, 222`). Both binaries resolve it as `unwrap_or_else(|| global_config.ffmpeg_path.clone())` → `ffprobe` points at the **ffmpeg** binary. `get_video_duration` then runs `ffmpeg` with ffprobe-only flags → ffmpeg prints "Unrecognized option", exits non-zero, stdout empty → `duration = stdout.trim().parse::<f64>().unwrap_or(0.0)` → `0.0` for **every** file. The check `input_duration > 0.0` is always false → the duration safeguard (headline correctness check against truncated conversions) is **silently skipped on the default config**. The `--watch` quick config sets `check_duration: Some(true)`, misleading users. Two wasted ffmpeg "ffprobe" spawns per video.

**Trigger:** Default config (no `ffprobe_path`); a file ffmpeg truncates (e.g. 30s output from 10m input). Check "passes", history records "done", corrupt half-video delivered.

**Fix:** See 02 §H5 — derive sibling ffprobe from `ffmpeg_path`; if ffprobe absent, log a warning and skip the duration check explicitly (don't masquerade as "checked, passed").

### H2 — Image conversion timeout does not actually cancel the work (`spawn_blocking` is uncancellable)
**File:** `src/processor/image.rs:70-76`; combined with `src/processor/runner.rs:39`.

`run_conversion` wraps the closure in `tokio::time::timeout`. For images the closure is `spawn_blocking` (sync `image` crate). `tokio::time::timeout` cancels by dropping the future; dropping a `spawn_blocking` `JoinHandle` does **not** abort the blocking task — it runs to completion. Consequences:
1. A timed-out image conversion keeps running arbitrarily long, occupying a blocking-pool thread. Sustained timeouts can exhaust the pool (default 512 threads).
2. The runner's timeout branch calls `cleanup_partial_output(&output_path)` and records an error; the still-running task may **write the output afterward** — a race leaving a "successful" file after an error was logged. Inconsistent history.

**Fix:** Either pre-check input size and bail synchronously before `spawn_blocking`, or move image transcoding to a subprocess (ffmpeg) so `.kill_on_drop(true)` + the outer timeout actually terminate it (consistent with every other processor). At minimum document non-cancellability + cap input size.

### H3 — Disk-low "pause" path silently drops the job: no dequeue, no error/history
**Files:** All processors' early return on low disk (`video.rs:32-38`, `audio.rs:31-37`, `image.rs:31-37`, `pdf.rs:30-36`, `document.rs:30-36`, `external.rs:30-36`), interacting with `processor/runner.rs:34-35` and `watcher/monitor.rs:209`.

On low disk each `process_*` does `if check_disk_space(...) { warn!(...); return; }` **before** `run_conversion`. The monitor already `enqueue(&file_name)`d (`monitor.rs:209`); on early return `health_server.dequeue` (runner.rs:35) is never called → the queue entry persists indefinitely (enqueue dedups). No history record, no error counter. Operators get only a `warn!`. Self-heals after space frees, but the dashboard lies in the meantime.

**Fix:** Route the disk-low case through `run_conversion` (closure returns explicit `Err("disk space low")`) so bookkeeping applies uniformly; or in the early-return path explicitly `dequeue` + `increment_error` + `add_history(status="skipped")` and leave `handle_input_file` alone (file should retry).

### H4 — `disk_space_monitor` conflates output/watch folders and is otherwise a no-op
**File:** `src/processor/disk.rs:117-135`, `check_disk_space` at `:6-45`.

`disk_space_monitor` calls `check_disk_space(folder, folder, &config)` (same string for **both** `output_folder` and `watch_folder`), so the `check_output`/`check_watch` flags gate the wrong list. The monitor discards the return value; `check_disk_space` only `warn!`s. The monitor is effectively dead code — the actual pause decision is made per-job in each processor.

**Fix:** Delete it (no real purpose) or have it set a shared `AtomicBool` consulted by processors (removes the duplicated `df` per conversion). If keeping, call `check_disk_space(folder, "", config)` for outputs and `("", folder, config)` for watch dirs.

---

## 🟡 MEDIUM

### M1 — PDF multi-output modes leave orphaned partial files on failure/timeout
**Files:** `src/processor/pdf.rs:170-191` (`extract_images`), `:278-299` (`pdf_to_images`); cleanup in `processor/runner.rs:48,86,105-113`.
`prefix = output.with_extension("")`; actual files are `prefix-NNN.png`. `cleanup_partial_output(output_path)` only `remove_file`s `output_path` which **never exists** for these modes → partial PNGs never cleaned, accumulate.
**Fix:** Generate to a temp dir, move on success / delete whole dir on failure; or teach `cleanup_partial_output` about prefix globs; or have the closure clean `prefix-*` on error before returning `Err`.

### M2 — `analyze_pdf` records success with a non-existent output path
**File:** `src/processor/pdf.rs:301-314`; via `runner.rs:66-80`.
`analyze_pdf` runs `pdfinfo` and writes nothing, but `run_conversion` records history with templated `output_path` and renames input to `.done`. `merge_pdfs` (`:274`) always bails (unimplemented). Also PDF default `output_ext` is `.pdf` regardless of `mode` → `extract_text` writes text into `*_converted.pdf`.
**Fix:** For non-file-producing modes record `output: ""`; set mode-aware default extensions (`.txt`, `.png`, …).

### M3 — `OutputNamer` path selection TOCTOU race between concurrent conversions
**File:** `src/processor/namer.rs:18-30`, `:42-49`.
Both helpers pick the first non-existent path with `if !path.exists() { return path; }`. Two concurrent conversions of different sources resolving to the same `base_name` can both observe the same slot free → second overwrites first's output; if the second errors, `cleanup_partial_output` deletes the first's valid output.
**Fix:** Pre-claim the slot with `OpenOptions::create_new(true)` (retry on `AlreadyExists`), or lock around `generate_path` per output folder. `processing_files` dedups only by input path — no help here.

### M4 — `HealthServer` uses `std::sync::Mutex` with `.lock().unwrap()`; poison cascade
**Files:** `src/processor/runner.rs:34,49,61,69,87,102` (`let _ = health_server.set_processing(...)` etc.); impl `src/health/server.rs:80,…`. *(Same as 05 §M7.)*
`let _ = …` discards `Result` but doesn't prevent poison panics from the `.lock().unwrap()` **inside** the methods. If a holder panics, the mutex poisons; the next processor call panics **before** `pf.lock().await.remove(&file_path)` (`main.rs:386`) runs → file stuck in `processing_files` permanently → permanent stall for that path. `add_history` also does `serde_json::to_string_pretty` + `std::fs::write` **while holding** the history mutex (widening poison surface, also blocks the async executor — 05 §M6).
**Fix:** Switch to `parking_lot::Mutex` (no poison) or recover with `lock().unwrap_or_else(|p| p.into_inner())`; move `fs::write` outside the lock (clone records, drop guard, then write).

### M5 — `validate_command_template` redundant with `build_argv` post-check
**File:** `src/processor/external.rs:162-166`.
The pre-check is redundant with `build_argv`'s per-token `..` check (and over-broad per C2). Consolidate traversal validation into one place (the output-path validator, per C1).

---

## 🟢 LOW

| ID | File:line | One-liner |
|----|-----------|-----------|
| L1 | image.rs:96-107,117-122 | Unknown `output_ext` → PNG encoded with wrong extension; use `save_with_format` consistently. |
| L2 | disk.rs:47-59 | `get_mount_point` misnamed (returns canonicalized path); rename `canonicalize_or_self`. |
| L3 | disk.rs:67-80 | `df` child lacks `.kill_on_drop(true)`; fail-open default defensible but add a debug log. |
| L4 | video.rs:252 | `pub async fn get_video_duration` only used in-module → `pub(crate)`/private. |
| L5 | external.rs:133-157 | `split_whitespace` can't express argv elements with literal spaces (executable path w/ space). |