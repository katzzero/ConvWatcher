# 05 — Health / Logs / Utils / Main Review

**Scope:** `src/health/{server,mod}.rs`, `src/logs/{error_logger,mod}.rs`, `src/utils/{path,hardware,mod}.rs`, `src/main.rs`, `src/lib.rs`  
~16 findings. See `00-index.md` for priorities.

---

## 🔴 CRITICAL

### C1 — `max_concurrent_conversions == 0` permanently deadlocks the worker pool
**File:** `src/main.rs:124-126` and `src/config/global.rs:178` (`max_concurrent_conversions: u32`).

`Semaphore::new(global_config.max_concurrent_conversions as usize)`. There is no lower-bound validation. serde only enforces `serde(default = "default_max_concurrent")` when the key is *absent*; an operator who explicitly sets `max_concurrent: 0` (or `max_concurrent_conversions: 0`) passes it straight through. `Semaphore::new(0)` is legal in tokio, and every job task does `sem.acquire().await` (`main.rs:385`).

**Trigger:** Operator sets `max_concurrent: 0`. Every queued conversion blocks on `acquire()` forever; no file is ever processed. `process_jobs` never logs anything (the recv loop fires, spawns tasks that hang), so the failure is silent — the daemon appears healthy (workers idle, queue growing) but does nothing. *(Same root as 02 §M5; the same bug exists in `src/bin/server.rs:115`.)*

**Fix:** Guard in `config::load_config` / `GlobalConfig` validation: `if max_concurrent_conversions == 0 { return Err("max_concurrent must be >= 1") }`, or clamp to `1` with a warning.

---

## 🟠 HIGH

### H1 — Only SIGINT is handled; SIGTERM (Docker's default stop signal) aborts without graceful shutdown
**File:** `src/main.rs:290-297`.

`tokio::select!` only awaits `tokio::signal::ctrl_c()` (SIGINT). No `SIGTERM` / `SIGHUP` handler. Docker (`docker stop`, orchestrators) sends **SIGTERM**, then SIGKILL after a grace period. On SIGTERM this code falls through `select!` only when the runtime itself is torn down — meaning the graceful-shutdown sequence at `main.rs:299` ("`shutdown_tx.send(())`", await monitors, abort tasks) is skipped entirely.

**Trigger:** `docker stop convwatcher` → process killed without draining in-flight conversions or sending the broadcast shutdown to monitors. Detached conversion tasks (`tokio::spawn` at `main.rs:384`) are cancelled mid-flight by runtime drop; ffmpeg children get `.kill_on_drop(true)` so they die, but history/`handle_input_file` bookkeeping is skipped → files may be left half-converted/unmarked.

**Fix:** Use `tokio::signal::unix` (`SignalKind::terminate()` + `SignalKind::interrupt()`) and race both in the `select!`:
```rust
let term = tokio::signal::unix::signal(SignalKind::terminate())?;
let int  = tokio::signal::unix::signal(SignalKind::interrupt())?;
tokio::select! { _ = term.recv() => ..., _ = int.recv() => ..., _ = mgmt_handle => ... }
```

### H2 — `app_log_path` is never set; `/logs` and `/logs/app` always return 404
**File:** `src/health/server.rs:26,66,258,302`; `src/main.rs:94-106`.

`HealthServer::new()` sets `app_log_path: None`. The only builder for log paths is `with_error_logger` (`server.rs:74`); there is **no `with_app_log` setter**, and neither `main.rs` nor `src/bin/server.rs` ever assigns `app_log_path`. Meanwhile `setup_logging` (`main.rs:335-356`) writes the app log to `./logs/convwatcher.log`. Result: the dashboard's `fetchText('/logs')` (`dashboard.html:160`) always receives a 404 "No log file configured", and the "Recent Logs" panel (`dashboard.html:210`) shows that string — **the endpoint is dead at runtime**.

**Fix:** Add `pub fn with_app_log(mut self, path: String) -> Self { self.app_log_path = Some(path); self }` and in `main.rs` chain `.with_app_log("logs/convwatcher.log".to_string())` (or derive from a config field). Same for `src/bin/server.rs`.

### H3 — `monitor_manager`: the top-level `shutdown_tx` is dead code; `mgmt_handle` is never awaited on shutdown
**File:** `src/main.rs:401,405,418,280-288,306-313`.

At `main.rs:401` `monitor_manager` creates `let (shutdown_tx, _) = broadcast::channel::<()>(1);` but **no monitor ever subscribes to it** — monitors subscribe to `new_shutdown_tx` (`main.rs:426`), created fresh inside the loop at `main.rs:418`. So the `let _ = shutdown_tx.send(())` at `main.rs:405` is a no-op (channel has no receivers; it silently ignores the `SendError`). Shutdown of the previous generation only happens because the loop iteration's `new_shutdown_tx` is *shadowed-and-dropped* at the next iteration's `let new_shutdown_tx = ...`, causing `Closed` on `shutdown_rx.recv()` (`monitor.rs:81` — the `select!` arm matches `_ = shutdown_rx.recv()` regardless of Ok/Err). That works **by accident**, not by design. On real shutdown (`main.rs:306-313`), `mgmt_handle` is neither `await`ed nor `abort`ed; `monitor_manager` exits only once `reload_rx` is closed — which happens when `config_reload_handle` and `embedded_scanner_handle` are aborted (`main.rs:309-310`) and drop their senders. At that point `new_shutdown_tx` is dropped and the last generation of monitors gets `Closed` — but `main` does not wait for them; `main.rs:313` returns and the runtime drops, cancelling the still-running monitor futures and any detached conversion tasks.

**Fix:** (a) Replace the dead `shutdown_tx` at line 401 with one that is actually reused across iterations (drop the inner `let new_shutdown_tx` per-iteration pattern or store the sender in a `let mut` that is reassigned). (b) After broadcasting shutdown in `main`, `await` `mgmt_handle` (or have `monitor_manager` return after its loop ends and the last generation is awaited) so reload-generation monitors are actually drained before the runtime drops.

### H4 — In-flight conversion tasks are detached and not drainable; aborting `worker_handle` does not stop them
**File:** `src/main.rs:384-388` (spawned per-job tasks), `main.rs:307` (`worker_handle.abort()`).

`process_jobs` schedules each job with a detached `tokio::spawn` (`main.rs:384`). `JoinHandle`s are dropped immediately, so on shutdown `worker_handle.abort()` only cancels the `job_rx.recv()` loop — it does **not** abort the already-spawned conversion tasks. They continue until the runtime is dropped (which aborts them hard). There is no `JoinHandle`-set to await. Contrast with AGENTS.md guidance that history records, `handle_input_file`, and partial-output cleanup should apply; on a hard runtime-drop these are skipped for in-flight jobs.

**Fix:** Collect per-job `JoinHandle`s in a `Vec` inside `process_jobs`, and on shutdown (after `job_tx` is dropped and `job_rx` returns `None`) `await` them with a bounded graceful timeout (`tokio::time::timeout`) before returning. Then have `main` await `worker_handle` instead of `abort()`-ing it.

---

## 🟡 MEDIUM

### M1 — `/api/watchers` JSON shape does not match the dashboard's expected schema
**File:** `src/health/server.rs:34-41,364-370` vs `src/health/dashboard.html:177-182`.

`WatcherInfo` serializes `{ name, watch_folder, output_folder, watch_type, rules: Vec<String> }` (a flattened `rules` list + `watch_type` string). The dashboard instead reads `w.video_rules`, `w.audio_rules`, `w.image_rules`, `w.pdf_rules`, `w.document_rules`, `w.custom_rules` (`dashboard.html:177-182`) — keys that never exist in the JSON. Every check `(w.video_rules || []).length > 0` is permanently false, so the rules section renders empty for every watcher. Also `w.name` is never displayed (the card title is `watch_folder`, `dashboard.html:174`).

**Fix:** Either change the dashboard to render `w.watch_type` + `w.rules` (join), or split `WatcherInfo.rules` into the typed `*_rules` arrays the JS expects. Pick one contract and document/test it.

### M2 — `/logs` and `/logs/app` read whole log files into memory; app log has no rotation → unbounded memory DoS
**File:** `src/health/server.rs:336-345` (`read_tail`), `server.rs:303` (`/logs/app` full read), `main.rs:335-356` (app log opened append-only, never rotated).

`read_tail` does `std::fs::read_to_string(path)` of the whole file then slices lines (`server.rs:337-344`) — the "100 lines" tail is purely client-side; the server still loads the entire file. `/logs/app` does a full `read_to_string`. The app log (`logs/convwatcher.log`) is opened with `create+append` and **never rotated** (only `ErrorLogger` rotates), so it grows without bound. A remote requester (the dashboard listens on `bind_address` from config, which may be `0.0.0.0` / exposed) can trigger repeated full-file reads of an arbitrarily large log → OOM or stalls on the blocking health thread.

**Fix:** Stream the tail with a bounded backward read (e.g. seek-from-end, or read last N KB), cap response size, and rotate the app log in `setup_logging` (size-based) the same way `ErrorLogger` does. Consider `tokio::task::spawn_blocking` + a hard size cap.

### M3 — tiny_http server has no read/idle timeout, header/body size cap, or connection limit
**File:** `src/health/server.rs:169-182`.

`tiny_http::Server::http(&addr)` is created with no `with_http_configs(...)` / no max header size, no body cap, no connection/request timeout, and the `for request in server.incoming_requests()` loop runs single-threaded on a `spawn_blocking` thread. A client that opens a connection and stalls (slowloris), or sends `Expect: 100-continue` with a huge `Content-Length` on a GET, can tie up the single serving thread indefinitely; the dashboard becomes unresponsive. tiny_http does not impose robust defaults here.

**Fix:** Configure `tiny_http::Server::http(addr).max_blocking_connections(...)` and add per-request body/header limits; wrap each request with a timeout; ideally run a small thread pool (`server.incoming_requests()` + `spawn` per request with a bounded timeout). Or replace tiny_http with hyper/axum on the existing tokio runtime for uniform async + timeouts.

### M4 — `enqueue` happens before `tx.send`; on send failure the queue entry is orphaned (queue drift)
**File:** `src/watcher/monitor.rs:209-213`.
*(Same as 03 §M4.)* Order is `health_server.enqueue(&file_name)` then `tx.send(job).await`. If `job_tx` is dropped, `tx.send` returns `Err`, the monitor `error!`s and removes from `processing_files` — but never `dequeue`s. That filename stays in `queue["global"]` forever; the dashboard queue view drifts on every send failure.
**Fix:** Call `enqueue` *after* a successful `tx.send`, or in the `Err` branch `let _ = health_server.dequeue(&file_name);`.

### M5 — `ErrorLogger::log` holds a `std::sync::Mutex` across blocking fs ops inside async context
**File:** `src/logs/error_logger.rs:25-40`.

`self.file.lock().unwrap()` is held across `fs::metadata`, `fs::rename`, `OpenOptions::open`, and `write_all` (all blocking). `ErrorLogger::log` is a sync fn invoked from async processors (`processor::runner` calls `error_logger.log(...)`). If the log directory lives on slow/networked storage, this stalls the tokio worker thread for the duration of fs I/O, every error event. Also (rotation): `let rotated = path.with_extension("log.old"); fs::rename(...)` keeps only **one** backup; the next rotation overwrites the previous `.log.old` → prior rotated errors are silently lost.

**Fix:** Either run the body inside `tokio::task::spawn_blocking`, or keep the critical section minimal (release the `Mutex` before doing I/O; the `Mutex` currently protects only a `PathBuf` that never changes, so it's essentially unnecessary and could be replaced by storing the path without a lock — the file I/O is itself serialized via the append-mode kernel lock only if path is stable). Add timestamped rotation names (`.log.1`, `.log.2`) or gzip old logs to preserve history.

### M6 — `add_history` is an `async fn` that performs no `.await` and holds a `std::Mutex` across a blocking file write
**File:** `src/health/server.rs:144-167`.

`pub async fn add_history` never awaits — misleading signature. Inside, `self.history.lock().unwrap()` (a `std::sync::Mutex`) is held across `serde_json::to_string_pretty` (full serialization of up to `max_records` entries — `O(max_records)` per call) **and** `std::fs::write(file, json)` (blocking). On every conversion completion this runs on the async executor (called from `processor::runner.rs:51,71,89`) — blocking I/O under the health `Mutex` stalls any thread touching the mutex (the dashboard's own request handlers also lock `history` at `server.rs:252`), and races with the bounded `spawn_blocking` health thread.

**Fix:** Make `add_history` synchronous (no await) and/or move the file write to `spawn_blocking`; serialize+write **after** dropping the lock (clone the `Vec` first, then write a snapshot). Better: maintain history in memory and persist asynchronously (debounced) rather than on every record.

### M7 — Pervasive `.lock().unwrap()` panics on mutex poisoning → cascade failure
**File:** `src/health/server.rs:80,91,100,116,122,128,137,145,213,214,215,233,240,241,252`; `src/logs/error_logger.rs:26`.
*(Same as 01 §M4.)* Every `std::sync::Mutex` guard `.unwrap()`s. If any code path panics while holding one of these locks (e.g. a future `.unwrap` on a `None` in conversion, or a poison from a panic in a task sharing the Arc), the mutex becomes poisoned and *every subsequent* `.lock().unwrap()` panics, taking down: enqueuing, dequeueing, history recording, and the dashboard handlers. Because conversions and the dashboard share these mutexes, a single panic disables the health subsystem for the process lifetime.

**Fix:** Handle poisoning explicitly (`lock().unwrap_or_else(|p| p.into_inner())` to recover, since the guarded data is simple), or switch to `tokio::sync::Mutex` / `parking_lot::Mutex` (no poisoning) where appropriate, and reduce panic-surface elsewhere so poisoning is unlikely.

### M8 — `health_handle.abort()` does not stop the underlying tiny_http blocking thread
**File:** `src/main.rs:112-120,306`; `src/health/server.rs:16,179-182`.

The health server runs as `tokio::task::spawn_blocking(move || hs.run())` (`main.rs:116`). `run()` is a blocking loop `for request in server.incoming_requests()` (`server.rs:182`). `health_handle.abort()` (`main.rs:306`) aborts the outer spawned task that is *awaiting* the `spawn_blocking` `JoinHandle`, but the blocking OS thread executing `tiny_http`'s accept loop is **not** interruptible — it keeps blocking on its own accept() until the process exits. So this is not actually graceful shutdown of the health server; it merely detaches the join.

**Fix:** Install a `running: AtomicBool` shutdown flag (the field already exists at `server.rs:16` but is never checked inside `run()`'s loop) and accept with a select/timeout, or close the listener. Currently `running` is set true at `server.rs:179` and **never read** — dead state. Either use it to break the loop or remove it.

---

## 🟢 LOW

| ID | File:line | One-liner |
|----|-----------|-----------|
| L1 | server.rs:225 | `/health` always returns `"disk_space": {}` (hardcoded empty). `disk_space_monitor` runs but never reports back. |
| L2 | server.rs:105-113 | Per-watcher counters are global; `_watcher` parameters unused. No per-watcher metrics exposed. |
| L3 | main.rs:317,335-356 | `setup_logging` uses relative `"logs"` path; no log rotation for the app log. AGENTS.md wants absolute. |
| L4 | server.rs:266-272,288-294,310-316 | Error 500 bodies use default `text/html` while interpolating an `io::Error`; keep `text_ct` on all responses. Should the message source ever surface a path → reflected-content sink. |
| L5 | server.rs:144 | `async` keyword superfluous (cf. M6). |
| L6 | server.rs:98-103 | `add_watcher_with_config` dedupes by `watch_folder`, silently replacing distinct watchers sharing a folder. Dedupe by `name`. |
| L7 | server.rs:22-23,106,111 | `AtomicU64` counters have no practical overflow; wrap semantics, fine in practice. |
| L8 | server.rs:89-93 | `with_history_persistence` drops the entire persisted history on any malformed record — `warn!` and/or recover the valid prefix. |
| L9 | server.rs:336-345 | `read_tail` joins with `\n` losing the original trailing newline (cosmetic). |
| L10 | server.rs:15-27,79 | `hw_info` behind Mutex but never exposed/read. Expose in `/health` (operators care about VAAPI/NVENC availability) or remove. |

---

## Best-practices / structural recommendations

1. **Runtime selection:** `#[tokio::main]` uses the multi-thread runtime by default with worker threads = CPU count — acceptable. But the health server runs on `spawn_blocking` (one thread) and blocks; consider moving it to an async framework (axum/hyper) so it integrates with the same runtime's timeout/backpressure, eliminating M3/M8.
2. **Blocking calls in async:** `std::sync::Mutex` + `std::fs::*` in `ErrorLogger::log`, `add_history`, and the health request handlers all execute on (or under) the tokio runtime. Use `spawn_blocking` for file I/O, or `tokio::sync::Mutex` + `tokio::fs`, and reserve std `Mutex` for trivially-short critical sections (M5, M6, M7).
3. **Shutdown ordering (proposed):** SIGTERM/SIGINT → set an atomic `shutting_down` → `drop(job_tx)` (so `process_jobs`'s recv returns `None`) → await `worker_handle` with a graceful timeout so in-flight jobs complete (or are timeout-cancelled via their existing 3600s guard) → broadcast monitor shutdown → await `monitor_handles` **and** `mgmt_handle`'s generation → close the health listener cleanly → exit. Currently `worker_handle`, `disk_handle`, `config_reload_handle`, `embedded_scanner_handle` are all `.abort()`ed (H4, M8) and `mgmt_handle` is not awaited at all (H3).
4. **Path validation:** The `/logs` endpoints read operator-controlled `error_log_path`/`app_log_path` strings with no canonicalization — they come from config (trusted-ish), but since AGENTS.md requires absolute paths for config, validate them at load (reject relative paths) the same way `watch_folder`/`output_folder` are validated. The `/logs` URL path is matched literally (`server.rs:258,302`) so there is **no path-traversal via the URL**; the only traversal surface is the config value itself.
5. **Stored-XSS verification (positive):** Confirmed safe. The dashboard (`src/health/dashboard.html`) escapes *every* interpolation into `innerHTML` via `escapeHtml(...)` — stats (`162-169`), watcher fields (`174-175`), rule arrays (`177-182`), processing map k/v (`192`), and history rows (`202-206`, including the `status-${…}` class attribute, where `"` escaping defeats attribute breakout). The `/logs` content is set via `textContent` (`dashboard.html:210`) — not `innerHTML` — so log lines (operator-influenced filenames) cannot inject script. The only html-injection-adjacent gaps are server-side error bodies with default `text/html` (L4), which are not currently attacker-controllable. **No stored-XSS found**, provided the `escapeHtml` discipline is maintained on future edits — the AGENTS.md note stands.
6. **`unwrap`/`expect`/`panic` hygiene:** Beyond M7's mutex unwraps, `tiny_http::Header::from_bytes(...).unwrap()` (`server.rs:189,193,196`) panics if those literal bytes were ever malformed (they're not, but it's compile-time-ish `unwrap`); and `serde_json::to_string_pretty(...).unwrap_or_default()` (`227,234,246,253`) returns `""` on the (impossible for these types) serialize error, yielding an empty 200 — acceptable. No outright `panic!`s found in these files.