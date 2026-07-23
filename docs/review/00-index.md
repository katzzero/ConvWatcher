# ConvWatcher — Code Review Report (Index)

**Reviewer:** Automated supervisor review (Rust)  
**Scope:** Full codebase — 42 `.rs` files, ~9,400 lines, 3 binaries (`convwatcher`, `convwatcher-server`, `convwatcher-agent`) + 1 lib + 1 common crate + tests.  
**Date:** Thu Jul 23 2026  
**Baseline:**  

| Check | Result |
|-------|--------|
| `cargo build` | ✅ clean |
| `cargo test --lib` | ✅ 41 passed |
| `cargo test` (e2e) | ✅ 2 passed |
| `cargo clippy --all-targets` | ⚠️ **31 warnings** |

**Method:** Five parallel deep reviews (processor / config / watcher / remote-worker / health+main), each file read top-to-bottom. Findings cross-referenced against `AGENTS.md`, module doc comments, and existing tests.

---

## Files in this report

- `01-processor.md` — `src/processor/` (runner, namer, video, audio, image, pdf, document, external, disk, job, mod)
- `02-config.md` — `src/config/`, `convwatcher-common/src/config.rs`, `src/cli.rs`
- `03-watcher.md` — `src/watcher/` (monitor, embedded)
- `04-remote-worker.md` — `src/worker/`, `src/bin/server.rs`, `convwatcher-common/`, `convwatcher-agent/`, `tests/`
- `05-health-and-main.md` — `src/health/`, `src/logs/`, `src/utils/`, `src/main.rs`, `src/lib.rs`

---

## 🔴 Three findings contradict explicit documentation (`[CONTRACT-BREAK]`)

These implement behavior that *contradicts* `AGENTS.md` or the modules' own doc-comments. Verify each against the line numbers before fixing.

1. **[04] RW C1** — Remote-failure / no-agent does NOT fall back to local processing. For `InputFileAction::Delete`, the input file is destroyed with no output — **silent data loss**.
2. **[04] RW C4** — Server-side timeout doesn't send `JobAbort`; agent ffmpeg keeps running. The `.kill_on_drop(true)` guarantee does not extend to remote conversions.
3. **[03] Watch H2** — Stability timer is NOT reset on size change, contradicting *"Size changes reset the timer"* (premature enqueue of half-written files).

---

## Executive priority table

| # | Sev | Area | One-liner | File |
|---|-----|------|-----------|------|
| 1 | 🔴 | processor/namer | Absolute `output_name` bypasses containment → arbitrary file write | 01 §C1 |
| 2 | 🔴 | config | `check_interval: 0` & `file_check_interval: 0` panic at startup ("0 disables" is false) | 02 §C1 |
| 3 | 🔴 | worker/dispatch | Remote-failure / no-agent does NOT fall back to local; `Delete` destroys input `[CONTRACT-BREAK]` | 04 §C1 |
| 4 | 🔴 | worker/coordinator | Stale handler evicts the live agent on reconnect / duplicate ID | 04 §C2 |
| 5 | 🔴 | worker/coordinator | No liveness probe; dead agents linger & get picked | 04 §C3 |
| 6 | 🔴 | worker/coordinator | Server timeout doesn't kill agent ffmpeg; `JobAbort` never sent `[CONTRACT-BREAK]` | 04 §C4 |
| 7 | 🟠 | config | Malformed `config.yaml` destructively overwritten with defaults | 02 §H1 |
| 8 | 🟠 | config | Documented `5Gb`/`10%` threshold forms can't deserialize → crash config load | 02 §H3 |
| 9 | 🟠 | config+proc | `ffprobe` fallback points at the `ffmpeg` binary → duration check silently never runs | 02 §H5 / 01 §H1 |
| 10 | 🟠 | proc/image | `spawn_blocking` image conversion is uncancellable; timeout still writes output afterward | 01 §H2 |
| 11 | 🟠 | watcher/monitor | `processing_files` leak for unmatched files → unbounded growth + permanent self-poisoning | 03 §H1 |
| 12 | 🟠 | watcher/monitor | Stability timer NOT reset on size change `[CONTRACT-BREAK]` | 03 §H2 |
| 13 | 🟠 | watcher/monitor | Shutdown blocked up to ~1h per watcher | 03 §H5 |
| 14 | 🟠 | worker/agent (pipe) | stderr never drained concurrently in pipe mode → deadlock >64 KiB stderr | 04 §H1 |
| 15 | 🟠 | worker/agent (pipe) | No overshoot guard in feed loop → `remaining` underflow | 04 §H3 |
| 16 | 🟠 | worker/agent | `output_ext` unsanitized → temp-file path traversal on agent | 04 §H4 |
| 17 | 🟠 | main | Only SIGINT handled; SIGTERM (Docker's signal) aborts without graceful shutdown | 05 §H1 |
| 18 | 🟠 | health | `app_log_path` never wired; `/logs` & `/logs/app` always 404 | 05 §H2 |
| 19 | 🟠 | main | `monitor_manager` dead `shutdown_tx`; `mgmt_handle` never awaited | 05 §H3 |
| 20 | 🟠 | main | In-flight conversion tasks detached, not drainable; `abort()` non-graceful | 05 §H4 |

(Full ~50 findings in the per-subsystem files.)

---

## Scope note for the builder — `embedded_secret` default (DECISION REQUIRED)

The reviewer presents **two options**; the builder should choose based on deploy context. Both options include the hardening from Proc C1 / RW H4 / Cfg M1,M3 / Watch M8 and adding `deny_unknown_fields` (Cfg M4) so a typo can't silently flip the secret.

### Option A — Hardening-only (keep by-design auth model)
Treat empty `embedded_secret` as intentional. Don't change the default; only harden path/output validation *around* it. Lowest operational impact: no first-run UX change, no secret rotation/recovery needed.

- **Pros:** No first-run UX change; operators opting into security set the secret explicitly.
- **Cons:** Default deploys remain unauthenticated for override/agent acceptance (documented, but the insecure default ships).

### Option B — Secure-by-default (recommended)
Auto-generate a non-empty `embedded_secret` on first-run generation (store it in `config.yaml`); overrides/agents authenticate by default. Document rotation/recovery.

- **Pros:** Closes the largest residual exposure by default; aligns default behavior with "secure out of the box".
- **Cons:** Changes first-run behavior (a generated secret in the config file); operators must retain it across restarts and share it with agents. Risk of lockout/rotation confusion in deployments that relied on the empty default. Document the migration (detect empty secret on upgrade → generate + `warn!`).

### Recommendation
The reviewer **recommends Option B** but leaves the call to the builder since it changes first-run UX and has an operations impact (forgotten secret vs. lockout) that is a product decision, not just a code defect.

This note is included verbatim in each subsystem file where the secret/auth surface appears (01, 02, 03, 04).

---

## Suggested fix order for the builder

1. **Startup-safety blockers** — 02 §C1 (interval `0` panic) + negative-duration + `max_concurrent: 0` deadlock (05 §C1). ~1h.
2. **Contract-breaks** — 04 §C1, 04 §C4, 03 §H2 (data-loss / contradict AGENTS.md). Add regression tests. ~1d.
3. **Agent pool liveness** — 04 §C2, 04 §C3 + wire `Heartbeat`/`JobAbort`. ~1-2d.
4. **Security: path traversal / RCE-adjacent** — 01 §C1, 04 §H4, 02 §M1, 02 §M3, 03 §M8 + 02 §M4 (`deny_unknown_fields`) + 02 §H4 (`bind_address` default). ~1d.
5. **Config robustness** — 02 §H1 (destructive overwrite), 02 §H3 (threshold forms), 02 §H5 (ffprobe). ~½d.
6. **Pipe mode** — 04 §H1 (deadlock), 04 §H3 (underflow), 04 §H2 (unbounded transfer). ~½d.
7. **Shutdown/drain** — 05 §H1 (SIGTERM), 05 §H3 (dead `shutdown_tx`), 05 §H4 (detached tasks). ~½d.
8. **Health wiring** — 05 §H2 (`/logs` dead), 05 §M1 (schema mismatch), 05 §M2 (whole-log reads), 05 §M3 (tiny_http timeouts). ~½d.
9. **Monitor correctness** — 03 §H1 (processing_files leak), 03 §H4 (double-processing), 03 §H7 (ext mismatch). ~½d.
10. **Concurrency hygiene** — mutex poison (01 §M4 / 05 §M7) + blocking I/O in async (03 §M3, 05 §M5, 05 §M6). ~1d.
11. **LOW items** opportunistically + CI lint gate (`cargo clippy --all-targets -- -D warnings` + `cargo fmt --check`).

---

## Clippy / lint baseline

`cargo clippy --all-targets` → **31 warnings**:
- `too_many_arguments` × 16 (e.g. `main.rs:364` `process_jobs`; group params into a struct)
- `derivable_impls` × 3
- `zombie_processes` × 1 (`tests/remote_worker_e2e.rs:89` — spawned agent never `wait()`ed)
- `ptr_arg`, `new_without_default`, `needless_borrow`, `manual_clamp`, `for_kv_map`, `doc_overindented_list_items` × 1 each

**No CI lint job exists** (CI builds Docker only). Consider adding `cargo clippy --all-targets -- -D warnings` + `cargo fmt --check` to CI so these are caught pre-merge.

---

## Stored-XSS verification (positive)

Verified safe. `dashboard.html` escapes **every** `innerHTML` interpolation via `escapeHtml(...)` (stats `:162-169`, watcher fields `:174-175`, rule arrays `:177-182`, processing map `:192`, history rows `:202-206` including the `status-${…}` class attribute — `"` escaping defeats attribute breakout). `/logs` content is set via `textContent` (`dashboard.html:210`), not `innerHTML`. No stored-XSS found, **provided the `escapeHtml` discipline is maintained on future edits** (AGENTS.md note stands). The only adjacent gap is server-side 500 bodies with default `text/html` (05 §L4) — not currently attacker-controllable.