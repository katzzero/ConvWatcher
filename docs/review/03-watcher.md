# 03 — Watcher Subsystem Review

**Scope:** `src/watcher/{monitor,embedded,mod}.rs`  
~14 findings. See `00-index.md` for priorities and the `embedded_secret` scope decision.

No `unwrap`/`expect`/`panic!` were found on production paths in these files (`unwrap_or`/`unwrap_or_default`/`unwrap_or(Path::new("."))` are all safe), so **no CRITICAL panics**. However, there are several **HIGH** correctness bugs in the stability state machine and the `processing_files` accounting.

---

## 🟠 HIGH

### H1 — Permanent `processing_files` leak for unmatched files (permanent self-poisoning)
**File:** `src/watcher/monitor.rs:190-215`.

On a stable file, `processing_files.insert(path)` (`:195`) runs *before* `create_job` is called (`:202`). When `create_job` returns `None` (file extension matches no rule — `.txt`, `.log`, `.lock`, `.part`, files in subfolders with no matching rule), the inner block is skipped, **no job is ever sent**, so the downstream worker (`main.rs:387`) never removes the entry. Meanwhile `file_states.remove(&path)` (`:215`) *is* reached, so the entry is dropped from `file_states`.

**Trigger:** Drop a non-matching file and leave it.
1. Scan N: stable → `processing_files.insert`, `create_job` → `None` → skip send → `file_states.remove`.
2. Scan N+1: `file_states` `None` branch → re-insert fresh `(Instant, size)`.
3. Scan N+2: stable again → `already_processing == true` (leaked) → `continue` (`:201`) which **skips** `:215`, so `file_states` lingers — and `processing_files` stays forever.
From scan N+2 on, the entry is permanently stuck in both maps. Each distinct junk file adds one permanent entry → **unbounded memory growth** in both `file_states` and `processing_files`. If a rule set is later hot-reloaded to match that extension, the file can never be re-enqueued (permanently marked "processing").

**Suggested fix:** Insert into `processing_files` only after a job is actually created and (preferably) sent; remove on `None`:
```rust
if let Some(job) = create_job(...) {
    let _ = health_server.enqueue(&file_name);
    if tx.send(job).await.is_err() {
        processing_files.lock().await.remove(&path);
    }
} else {
    processing_files.lock().await.remove(&path);
}
file_states.remove(&path);
```
Or move the `processing_files.insert` *inside* the `if let Some(job)` block after send success.

### H2 — Stability timer is NOT reset on size change (contradicts spec) `[CONTRACT-BREAK]`
**File:** `src/watcher/monitor.rs:187-188`.

When a periodic scan detects a size change, `first_seen` is preserved and only `last_size` is updated:
```rust
if current_size != last_size {
    file_states.insert(path.clone(), (first_seen, current_size));   // keeps first_seen!
} else if first_seen.elapsed() >= stable_time {
    ...enqueue...
}
```
AGENTS.md explicitly states: *"Size changes reset the timer."* The code does the opposite. The enqueue condition is `time since **first** seen >= stable_time`, not `time since last size change >= stable_time`.

**Trigger:** A tool creates a zero-byte / pre-allocated placeholder file at t=0, then writes content later (common with download managers, SFTP staging, encoders that open output then stream in). With `stable_time=5s`, `check_interval=1s`: scans at 1..5s size 0 unchanged → at t=5s `first_seen.elapsed()>=5s` → **enqueue a 0-byte file** while the writer is still about to write. Also bites: a file paused mid-copy (size unchanged for > `stable_time`), then resumes → enqueued prematurely during the pause.

**Suggested fix:** Reset `first_seen` on size change, matching the documented behavior:
```rust
if current_size != last_size {
    file_states.insert(path.clone(), (Instant::now(), current_size));
}
```

### H3 — Inconsistent stability semantics between `notify` path and `scan` path
**File:** `src/watcher/monitor.rs:88-90` vs `187-188`.

The notify-event arm always writes `(Instant::now(), size)` — i.e. **resets the timer** on every event. The periodic-scan size-change arm keeps `first_seen` (per H2, doesn't reset). So whether a file gets enqueued "stable_time after first appearance" or "stable_time after last modification" depends on **whether `notify` events happen to be delivered** for that file — different on platforms with sparse event coalescing, on overloaded inotify/fsevents, or when `notify`'s internal queue overflows (errors silently dropped at `monitor.rs:49`).

**Fix:** Pick **one** definition of "stable" and apply it uniformly. Recommended: reset `first_seen` on size change in *both* paths (fixes H2 and H3 together).

### H4 — `already_processing` branch keeps the `file_states` entry → double-processing
**File:** `src/watcher/monitor.rs:199-201`.
```rust
if already_processing {
    continue;          // skips file_states.remove(&path) at line 215
}
```
`continue` jumps past `file_states.remove(&path)` (`:215`), so the entry lingers with its now-old `first_seen`/size. The next time the *current* job finishes and the worker removes the path from `processing_files` (`main.rs:387`), the very next scan sees the lingering `file_states` entry: size unchanged, `first_seen.elapsed() >= stable_time` (it's been waiting the whole conversion), `already_processing == false` now → **re-enqueue**. This happens whenever a trailing `notify` `Modify` event (or any `event_rx` event) re-added the path to `file_states` during the first conversion (common — encoding tools often stat/rewrite the input).

**Fix:** When `already_processing`, *remove* the entry instead of keeping it:
```rust
if already_processing {
    file_states.remove(&path);
    continue;
}
```

### H5 — Shutdown can be blocked up to the conversion timeout (~1h) per watcher
**File:** `src/watcher/monitor.rs:95-110`, `:210`.

`scan_directory` is `await`ed inside the `scan_ticker.tick()` arm of `tokio::select!`. While inside it, `shutdown_rx.recv()` is not polled. `tx.send(job).await` (`:210`) blocks until the 100-deep job channel has capacity. If the worker pool is saturated with long conversions (each up to the 3600s timeout in `processor::runner`), the channel stays full → `tx.send` blocks → the select arm never returns → **shutdown is delayed up to ~1 hour per watcher**, and even longer because monitors are restarted serially in `monitor_manager` (each old handle `await`ed sequentially, `main.rs:407-409`). `cleanup_stale_entries` is also skipped during that wait.

**Fix:** Race the send against shutdown, or use `try_send` with backoff:
```rust
tokio::select! {
    biased;
    _ = shutdown_rx.recv() => { processing_files.lock().await.remove(&path); break; }
    res = tx.send(job) => { if res.is_err() { processing_files.lock().await.remove(&path); } }
}
```
Also pass `shutdown_rx` into `scan_directory` (or break the scan into cancellable chunks).

### H6 — Promotion writes to a relative `config/watchs` path (violates project rule)
**File:** `src/watcher/monitor.rs:274`.
`let watchs_dir = PathBuf::from("config/watchs");` is a relative path. AGENTS.md: *"All paths must be absolute — relative paths are rejected at startup."* The monitor bypasses that and makes the promoted config depend on the process **CWD**. If the daemon is launched from a different working directory, the promoted override lands in the wrong place and is never picked up — silently breaking the entire override flow. Promotion "succeeds" (no error, `info!` says it worked), but the embedded scanner never sees the file. *(Same root cause as 02 §M2.)*

**Fix:** Resolve the watchs dir against the config base path (the same base used by `config::load_config`), or accept an absolute `watchs_dir` parameter in `run_file_monitor` and pass it down.

### H7 — Subfolder rules match on `subfolder` alone, ignoring `input_extensions`
**File:** `src/watcher/monitor.rs:329-360`.

In `find_matching_rule`, the subfolder branch only tests `r.subfolder.as_deref() == Some(fmt)` — it does **not** verify `r.input_extensions.contains(&file_ext)`. Any file placed in a `->gpu` subfolder matches the GPU rule regardless of extension. A stray `clip.txt`, `thumbs.db`, `.lock`, or `.part` file in `->gpu/` creates a `ConversionJob` → ffmpeg is launched on the junk file → conversion fails → error history + wasted queue/semaphore slot.

The unit test `test_create_job_no_match_wrong_subfolder` only "passes" because the chosen subfolder `unknown` has no rule, so it falls through to the (failing) extension check; it never exercises the case where a subfolder rule *exists* but the extension mismatches.

**Trigger:** `->gpu/notes.txt` in a video watcher with only `input_extensions: [".mxf"]`.

**Fix:** Also require extension match in the subfolder branch:
```rust
.find(|r| r.subfolder.as_deref() == Some(fmt) && r.input_extensions.contains(&file_ext))
```
Add a unit test for a known subfolder with a non-matching extension.

---

## 🟡 MEDIUM

### M1 — Silent `read_dir` failure: daemon silently stops watching
**File:** `src/watcher/monitor.rs:125-128`.
`Err(_) => return` swallows permission/IO errors with no log and no health signal. If the watch folder becomes unreadable (chmod, remount, NFS hiccup, root-only perms), every scan silently aborts and the daemon looks "alive" (health endpoint green) while processing nothing.
**Fix:** `warn!`/`error!` the error, and consider surfacing it to `HealthServer` (an "errors" counter is already there).

### M2 — Unmatched stable files cause perpetual re-stable churn (paired with H1)
**File:** `src/watcher/monitor.rs:202-215`.
Even after fixing H1, a stable file that matches no rule is removed from `file_states` every cycle and re-added next scan → check-stable → `create_job None` → remove → repeat forever. Each cycle does `metadata()` + match + create_job work for every junk file. On a busy drop dir this is wasted CPU every `check_interval`.
**Fix:** Treat "no matching rule" as a terminal state — leave the entry in `file_states` (so it doesn't re-stable each scan) and only re-evaluate on a `notify` event or `mtime`/size change. (Dovetails with H3's "reset on size change" model.)

### M3 — Synchronous filesystem I/O on the async task
**File:** `src/watcher/monitor.rs:88-89, 125-182, 226-296, 469`.
The monitor task runs on a tokio worker thread and performs `std::fs::metadata`, `read_dir`, iterates `entries.flatten()` doing per-entry `metadata()`, plus `read_to_string` / `fs::copy` / `fs::rename` / `fs::write` for override promotion, and `path.exists()` per entry in `cleanup_stale_entries`. None of these use `spawn_blocking`. On watch folders with thousands of files this blocks a tokio worker thread for the full scan duration, starving other tasks (the job processor, health server). AGENTS.md flags the image processor as the only `spawn_blocking` user — the monitor is the bigger offender by far.
**Fix:** Wrap `scan_directory` (and especially `cleanup_stale_entries`'s bulk `exists()` calls) in `tokio::task::spawn_blocking`, returning the list of stable candidates to the async task, which then performs the `processing_files.lock().await` / `tx.send().await` work. Promotion (`copy`/`rename`) should likewise be `spawn_blocking`.

### M4 — `health_server.enqueue` runs before `tx.send`, orphaning queue entries on send failure
**File:** `src/watcher/monitor.rs:209-213`.
`enqueue(&file_name)` is recorded into the global health queue, then `tx.send` may fail. On failure the path is removed from `processing_files` and from `file_states`, but the queue entry is never `dequeue`d → the dashboard `/api/queue` shows files stuck "queued" forever, growing over time whenever the worker pool goes away. *(Same as 05 §M4.)*
**Fix:** Reverse the order: `tx.send(job).await` first; on success call `enqueue`; on failure remove from `processing_files` and do not enqueue. Or call `health_server.dequeue(&file_name)` in the `is_err()` branch.

### M5 — Embedded scanner: ignored `reload_tx.send` failure, no graceful shutdown
**File:** `src/watcher/embedded.rs:93` and `:230-235`.
- `let _ = self.reload_tx.send(merged).await;` — if `monitor_manager` (the receiver) has exited or the channel is closed, the merged config update is silently lost; there is no `error!`/`warn!`.
- `run_embedded_scanner` is an infinite `loop { sleep; scan }` with no shutdown signal. On process shutdown it keeps scanning until the runtime drops it; paired with the monitor shutdown flow it isn't coordinated.

**Fix:** Log on send error (`warn!`); thread a `broadcast::Receiver<()>` shutdown into `run_embedded_scanner` and break out of the loop, ideally using `tokio::select!` between `sleep` and `shutdown_rx.recv()`.

### M6 — Embedded scanner can parse a half-written override (copy is non-atomic)
**File:** `src/watcher/embedded.rs:131` (vs `src/watcher/monitor.rs:281`).
Promotion uses `std::fs::copy(config_path, &dest)` (non-atomic) followed by `rename(config_path, old_path)`. The embedded scanner periodically `read_dir`s `config/watchs/` and parses `*.yaml`; if its scan lands while `fs::copy` is mid-write, `serde_yaml::from_str` fails on a truncated YAML → the scanner logs `warn!` and does not re-attempt until the file's mtime changes again (`known_configs` keyed by mtime). Usually self-heals on the next scan, but transient parse failures and missed merges can occur under heavy churn.
**Fix:** Promote with an atomic write into a `*.yaml.tmp` then `rename` onto the final name (rename is atomic on the same filesystem). The scanner will then only ever see complete files.

### M7 — `cleanup_stale_entries` calls `path.exists()` per entry (sync I/O, O(n) per scan)
**File:** `src/watcher/monitor.rs:466-474`.
`retain`'s closure does `!path.exists()` (a `stat` syscall) for *every* tracked path on every scan tick. For long-lived watchers with thousands of tracked paths this is O(n) sync syscalls fired on the async task every `check_interval`, compounding M3. Additionally, the eviction age is `stable_time * 10` (50s default) — a legitimately slowly growing file (e.g. a multi-hour network copy that pauses >`stable_time` but resumes) gets evicted every 50s and re-added fresh, which interacts with H2/H3 to cause either premature enqueue or extra work.
**Fix:** Move cleanup into `spawn_blocking`. Consider basing "stale" on "size unchanged for K consecutive scans + age" rather than a fixed multiple, or evict only on `notify::EventKind::Remove` events (which the watcher already receives — currently ignored at `monitor.rs:58`).

### M8 — Override promotion is an RCE / injection surface with weak validation
**File:** `src/watcher/monitor.rs:225-296` (promote), `:243-261` (empty-secret accept).
When `embedded_secret` is empty (the documented default), any file dropped into the watch folder named `<watcher>.yaml` is promoted and accepted — and custom presets run arbitrary CLI (AGENTS.md flags this as RCE surface). Beyond the documented warning, there is:
- No path sanitization of `manifest.name` before `watchs_dir.join(format!("{}.yaml", manifest.name))` (line 280). A watcher named `../foo` would write outside `config/watchs/`.
- The merged config fully *replaces* `watch_type` with the override's (`embedded.rs:159`) — a compromised override can swap a watcher's preset to any preset name (including custom CLI presets), since rules are not re-validated post-promotion.

**Trigger:** Any agent with write access to the watch folder can inject/replace rules, run arbitrary CLI on the server. The lack of `manifest.name` sanitization is an unflagged escalation.

**Fix:** Validate that `manifest.name` is a flat identifier (regex `^[A-Za-z0-9_.-]+$`, no `/`/`..`); canonicalize `watchs_dir` and verify the destination stays within it (mirror `processor::namer::validate_output_path`). Re-validate merged rules at promotion time.

---

## 🟢 LOW

| ID | File:line | One-liner |
|----|-----------|-----------|
| L1 | monitor.rs:281-287 | Non-atomic two-step promote (copy then rename original to `.old`); if killed mid-way, re-promotes noisily. Use `rename` directly. |
| L2 | monitor.rs:317-320 | Extension matching case-sensitive on the rule side (`".MP4"` never matches); lowercase rules or document. |
| L3 | monitor.rs:322-327 | `->` routing only one level deep; nested deeper falls through to root rules; document or walk parents. |
| L4 | monitor.rs:49 | `notify` errors (buffer overflow) silently dropped → `warn!` to detect backpressure. |
| L5 | monitor.rs:74 | `interval` default `Burst` → scan bursts after a slow scan; use `MissedTickBehavior::Delay`. |
| L6 | main.rs:384-388 | Detached per-job `JoinHandle`; panic swallowed, no dequeue/history/pf-remove; `catch_unwind` or `await`+`is_panicked`. |
| L7 | embedded.rs:41-49 | `try_recv` drain keeps only the last config snapshot; a transient empty `Vec` clears `known_configs` → full re-merge/re-broadcast. |
| L8 | monitor.rs:74-79 | First `interval` tick immediate → startup scan races `notify` registration. Cosmetic. |

---

## Positive observations

- The stability state machine uses `Instant` (monotonic) — correct choice, no wall-clock drift.
- `file_states` is only mutated inside the single async task's `select!` arms (the notify path goes through a channel, the scan path is direct) — **no shared-state race on `file_states`** and no lock needed. Good design.
- `processing_files.lock()` is released before `tx.send().await` — no lock held across the channel await, avoiding the obvious deadlock.
- `find_matching_rule` correctly prefers `->` subfolder rules and falls back to generic extension rules; `cleanup_stale_entries` also evicts deleted paths (good safety net).
- Unit tests cover extension matching, subfolder matching, no-match, and folder creation — good (though H7 shows a missing case).

## Top recommended fixes (priority order)
1. **H1** — only mark `processing_files` for actually-created jobs; remove on `create_job None`. Prevents memory leak + permanent self-poisoning.
2. **H2 + H3** — reset `first_seen` on size change in both notify and scan paths. Restores documented semantics, eliminates premature enqueue.
3. **H4** — `file_states.remove(&path)` in the `already_processing` branch. Removes double-processing.
4. **H5** — race `tx.send` against `shutdown_rx` (or pass shutdown into `scan_directory`). Removes up-to-1h shutdown stall.
5. **H6** — absolute `watchs_dir` (resolve against config base, not CWD).
6. **H7** — require `input_extensions` match in subfolder rule branch; add a regression test.
7. **M3/M7** — `spawn_blocking` for `scan_directory`/`cleanup_stale_entries`.
8. **M4** — reorder `enqueue` after successful `tx.send`.
9. **M1** — log `read_dir` errors instead of silent `return`.