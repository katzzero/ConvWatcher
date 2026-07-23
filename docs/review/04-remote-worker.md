# 04 — Remote Worker Subsystem Review

**Scope:** `src/worker/{dispatch,coordinator,mod}.rs`, `src/bin/server.rs`, `convwatcher-common/src/{protocol,transport,discovery,ffmpeg,lib,config}.rs`, `convwatcher-agent/src/{main,runner}.rs`, `tests/remote_worker_e2e.rs`  
~15 findings. See `00-index.md` for priorities and the `embedded_secret` scope decision.

---

## 🔴 CRITICAL

### C1 — Remote-failure path does NOT fall back to local processing `[CONTRACT-BREAK]`
**File:** `src/worker/dispatch.rs:194-236` (and `route_job` at `:34-89`).
AGENTS.md: *"everything else … and any no-agent / remote-failure case falls back to local `processor::process_one`."* The module doc (`dispatch.rs:8-10`) repeats it. The code does not implement this.

`run_remote` wraps the remote attempt in `run_conversion(...)` with a closure that returns `Err` on both `Ok(false)` ("no agent available") and `Err(e)` (remote failure) — `dispatch.rs:228-232`:
```rust
match pool.dispatch(remote).await {
    Ok(true) => Ok(output_for_convert.to_string_lossy().to_string()),
    Ok(false) => Err(anyhow::anyhow!("no agent available")),
    Err(e) => Err(e),
}
```
`run_conversion` (`src/processor/runner.rs:66-99`) treats any `Err` as a definitive conversion failure: increments the error counter, records history `status:"error"`, runs `cleanup_partial_output`, and calls `handle_input_file(&file_path, &action, false)`. There is no subsequent call to `crate::processor::process_one`.

**Trigger scenarios:**
1. Remote job genuinely fails transiently (agent ffmpeg crashes, network blip, oversized input) → the conversion is logged as **failed**, the input file is **marked/deleted per `InputFileAction`**, and NO local retry happens. For `InputFileAction::Delete` the **source file is destroyed with no output produced — silent data loss.**
2. Agent count was >0 at `route_job`'s `agent_count()` check (`dispatch.rs:43`) but the agent disconnected before `pool.dispatch` acquired the lock (TOCTOU) → `dispatch` returns `Ok(false)` → same failure + input-deletion outcome.

The existing e2e test `no_agent_falls_back_to_local` (`tests/remote_worker_e2e.rs:157-179`) only asserts `WorkerPool::dispatch` returns `Ok(false)` — it does **not** exercise `route_job`'s fallback, so this bug is uncaught.

**Suggested fix:** Restructure so the remote attempt is decoupled from `run_conversion`'s error bookkeeping. Either:
- Attempt `pool.dispatch` first; on `Ok(true)` wrap in `run_conversion` for success bookkeeping; on `Ok(false)` *or* `Err` call `processor::process_one` (which runs its own `run_conversion`). Or
- Add a `remote_attempt()` that returns `Result<bool>` and have `run_remote` perform local fallback before invoking the shared bookkeeping.

### C2 — Reconnecting/duplicate-ID agent evicts the LIVE agent from the pool (race)
**File:** `src/worker/coordinator.rs:203-221`.

`handle_agent` inserts the agent into the map under `agent_id` (`:203-206`), then runs a monitor loop that only exits when `alive` becomes false (`:211-217`), then unconditionally removes `agent_id` from the map (`:219`).

Two problems:
1. **Stale-handler never exits.** The monitor loop only observes `alive` being flipped to `false` by a *failed dispatch* (`coordinator.rs:252`). If an agent's TCP connection dies silently while no job is in flight (agent process killed, network drop, TCP RST not yet noticed), `alive` stays `true` forever → the handler loop sleeps 5s and rechecks forever; the dead `Arc<Agent>` remains in the map. The dead connection's reader/writer are kept alive by the Arc.
2. **Re-registration replaces then stale-removal evicts the NEW agent.** If an agent reconnects (common: the agent `main.rs:189-194` retries in a 3s loop after any disconnect), its new `handle_agent` task does `insert(agent_id, new_agent)`, **replacing** the stale entry. The stale task still holds an `Arc<Agent>` to the old connection; its monitor loop still sees old `alive=true`; it eventually calls `agents.lock().await.remove(&agent_id)` — **removing the new, live agent**. From the pool's view, the agent vanishes despite being connected. The same applies to two agents sharing a hostname (`agent_id` defaults to `HOSTNAME` env, `convwatcher-agent/src/main.rs:90-95`).

**Fix:** Make the monitor loop own the registration lifetime: after `insert`, on monitor exit (whether `alive=false` OR a real liveness probe fails), `remove` **only if the map still holds our `Arc<Agent>`** (compare `Arc::ptr_eq`), or use a generation token. Better: use a per-handler `JoinHandle`/`CancellationToken` and have the monitor proactively detect dead sockets (read with timeout / TCP keepalive / heartbeat) rather than relying on `alive` flag.

### C3 — No real liveness detection; dead agents stay "least-loaded" and get picked forever
**File:** `src/worker/coordinator.rs:208-264`.

The "health-monitor loop" (`:208-217`) is misnamed — it never probes the connection. It only polls an `alive` flag that is set to `false` solely by a failed `dispatch` (`:252`). `pick_agent` (`:258-264`) selects `min_by_key(inflight)` with **no `alive` filter** and no recency check.

So: an agent whose socket is dead but on which no dispatch has failed yet (e.g. agent killed between jobs) has `inflight=0` and is always selected as least-loaded. The first `dispatch` to it will write the job header, immediately fail, mark `alive=false`, and bubble `Err` — but per C1 that error is recorded as a conversion failure (no fallback). Until a job arrives, the dead agent lingers indefinitely.

`Message::Heartbeat` is defined (`protocol.rs:108`) but the server never sends one and the agent only *echoes* inbound heartbeats (`agent main.rs:275-277`); there is no proactive heartbeat either way.

**Fix:**
- Enable `TCP_KEEPALIVE` on the accepted stream and have the monitor actually `read_message(&mut reader)` with a timeout — an EOF/error means the socket is gone.
- Send periodic `Heartbeat` from the server; if the agent fails to reply within N intervals, mark `alive=false` and evict.
- Filter `pick_agent` by `alive`.

### C4 — Server-side timeout does not abort the agent's ffmpeg (orphaned work + resource waste) `[CONTRACT-BREAK]`
**File:** `src/worker/coordinator.rs:266-348`, `src/processor/runner.rs:38-64`.

`run_conversion` wraps the remote `convert()` in `tokio::time::timeout(3600s)` (`runner.rs:38-39`). On timeout, the future is dropped. Dropping the future cancels the in-flight `read_message`/`read_stream` awaits in `run_on_agent`, but the server **never sends `Message::JobAbort`**. `JobAbort` is defined in the protocol (`protocol.rs:104-105`) and handled by the agent (`agent main.rs:278-282`) but the agent handler is a no-op ("Jobs run synchronously in this loop; an abort for a finished job is a no-op") and no one ever sends it anyway.

Result: on a server-side timeout the agent keeps running ffmpeg to completion. If the output fits in the TCP send buffer, ffmpeg finishes, the agent sends `JobOutputStart`+output+`JobResult` into a socket the server is no longer reading — those bytes are buffered until the OS drops the connection, then the agent's `write_stream` errors out and the agent marks the job failed and moves on. For small outputs this means a "timed out" job actually completed on the agent (wasted CPU + the output is discarded). For large outputs the agent eventually errors on write. Either way the server-side `.kill_on_drop(true)` guarantee (AGENTS.md: "keep the `.kill_on_drop(true)` on every `tokio::process::Command`") does NOT extend to the remote ffmpeg — there's no mechanism to kill it.

**Fix:** Before/after `run_conversion`'s timeout branch (or inside a wrapper around `run_remote`), send `Message::JobAbort { job_id }` over the connection (requires not holding the conn mutex past the timeout point, or a side-channel). The agent should track the active job id and, on `JobAbort`, kill the running child (which `kill_on_drop(true)` would also do if the agent task were aborted — but the agent task isn't).

---

## 🟠 HIGH

### H1 — Pipe-mode deadlock: stderr pipe never drained concurrently
**File:** `convwatcher-agent/src/runner.rs:187-248`.

`run_pipe` spawns ffmpeg with `stdin`/`stdout`/`stderr` all piped. It concurrently runs `feed` (read socket → write stdin) and `drain` (read stdout → `out_buf`), but **stderr is only drained after `child.wait()`** (`:242`):
```rust
let mut err_buf = String::new();
stderr.read_to_string(&mut err_buf).await.ok();
let status = child.wait().await...
```
ffmpeg writes all progress/statistics to **stderr** by default (one line per ~500ms with `-stats`). The OS pipe buffer is typically 64 KiB. Once stderr fills, ffmpeg blocks writing to stderr, which blocks it from writing more stdout, so `drain` (`read_to_end` on stdout) stalls forever. `feed` finishes stdin and waits on `drop(stdin)`; `tokio::join!` never completes → **deadlock** for any conversion producing > ~64 KiB of stderr (long videos, verbose codecs, repeated warnings).

**Trigger:** Any pipe-mode job whose ffmpeg stderr exceeds the pipe buffer (≈ a video longer than a handful of seconds with default stats).

**Fix:** Drain stderr concurrently as a third task in the join (or `tokio::spawn` a stderr→String task), e.g.:
```rust
let err_task = async { stderr.read_to_string(&mut err_buf).await };
let (feed_res, drain_res, _err_res) = tokio::join!(feed, drain, err_task);
```
Reproducible right now but the e2e test only exercises `temp` mode (`tests/remote_worker_e2e.rs:104` passes `--io-mode temp`), so the deadlock is not caught.

### H2 — No cap on total transfer size → disk/memory exhaustion DoS
**Files:** `convwatcher-common/src/transport.rs:19,76-122`, `src/worker/coordinator.rs:270-291,326-328`.

`MAX_FRAME_LEN` (8 MiB) bounds *per-frame* allocation, but `write_stream`/`read_stream` honor `total: u64` with **no upper bound**. The `Job` message carries `input_len: u64` (server-chosen from file metadata) and the agent replies with `JobOutputStart { output_len: u64 }` (agent-chosen). Neither side validates against a configurable maximum.

- A hostile/buggy agent can announce `output_len = 10 TiB`; the server's `run_on_agent` will `File::create(job.output_path)` and loop `read_stream` filling disk until ENOSPC. Since the server's `DiskSpaceConfig` check (`dispatch.rs:99`) happened before the transfer, this bypasses it.
- A rogue coordinator can likewise ask a trusting agent (empty secret → accepted, AGENTS.md "warn + accept") to receive `input_len = 10 TiB` into `temp_dir` → agent disk exhaustion.
- In pipe mode the agent buffers the **entire output** in `out_buf: Vec<u8>` (`runner.rs:229-235`) — any large output exhausts agent RAM regardless of total-size cap (see L4).

**Trigger:** Compromised peer, or a benign peer with a corrupted/garbled `output_len`/`input_len`.

**Fix:** Add a configurable `max_transfer_bytes` (per-job and global) enforced in both `write_stream`/`read_stream` and in `run_on_agent`/`run_temp`/`run_pipe` before allocating/streaming.

### H3 — Pipe-mode feed loop has no overshoot guard → `remaining` underflow
**File:** `convwatcher-agent/src/runner.rs:210-222`.

The `feed` task manually re-implements frame reading:
```rust
while remaining > 0 {
    let chunk = read_frame(reader).await?;
    ...
    stdin.write_all(&chunk).await...?;
    remaining -= chunk.len() as u64;   // <-- underflow if chunk.len() > remaining
}
```
Unlike `transport::read_stream` (which explicitly checks `chunk.len() as u64 > remaining` at `transport.rs:110-116`), the pipe-mode loop has **no overshoot guard**. A malformed/rogue server sending a frame larger than `remaining` causes `remaining` to underflow: in release builds (`u64` wrapping subtraction) `remaining` wraps to a huge value, the loop continues, and the agent feeds arbitrary extra bytes into ffmpeg stdin — wasting CPU/IO and possibly never terminating (reads keep succeeding until socket EOF).

**Trigger:** Server sends an input frame larger than the announced `input_len` (rogue or bug).

**Fix:** Use `transport::read_stream(reader, &mut stdin_proxy, input_len)` (write to stdin via an adapter), or add the same `chunk.len() as u64 > remaining` bail present in `read_stream`.

### H4 — Path traversal in temp-file naming via attacker-controlled `output_ext`
**File:** `convwatcher-agent/src/runner.rs:117-121`.
```rust
let out_path = base.join(format!("cw-{job_id}-out.{}", output_ext.trim_start_matches('.')));
```
`output_ext` is server-supplied (ultimately from `WireVideoRule`'s neighbor `output_ext: String` in `protocol.rs:82`, echoed by `coordinator.rs:288`). It is joined into a path with **no sanitization** beyond stripping leading dots. `File::create(&out_path)` (and the ffmpeg OUTPUT_TOKEN substitution) will follow `/` and `..` separators to write outside `temp_dir`. While `format!("cw-{job_id}-out.{ext}")` prepends one path component, that only offsets traversal by one level; a value like `output_ext = "../.bashrc"` (or any crafted path with enough `..`/abs-path mixes) can land writes outside `temp_dir` (e.g. overwrite `~/.bashrc`, `~/.ssh/authorized_keys`, cron dirs). The `in_path` (`runner.rs:117`: `"cw-{job_id}-in.tmp"`) is fixed and safe; only `out_path` is vulnerable.

**Trigger:** A rogue coordinator with the empty-secret default (AGENTS.md: "overrides accepted WITHOUT auth") connecting to an agent, or simply a misconfigured `output_ext` containing path separators.

**Fix:** Sanitize `output_ext` to a strict charset (e.g. `^[A-Za-z0-9_+-]+$`, ≤16 chars) before interpolation; reject the job otherwise. Or always use a fixed suffix (`cw-{job_id}-out.bin`) for the *local* temp file and pass the real extension only to ffmpeg via its own output-options (ffmpeg infers container from extension, so this may require `-f` explicit).

---

## 🟡 MEDIUM

### M1 — Discovery has no rate limiting / backoff → beacon broadcast storm
**Files:** `convwatcher-common/src/discovery.rs:25-49` (server), `:71-93` (agent).
- `serve_discovery` answers every `Beacon` it receives with no per-source rate limit → any LAN host can spam beacons and pin the coordinator's CPU/IO. There's no source deduplication or quota.
- `discover_coordinator` broadcasts every `broadcast_interval` (3s in `agent main.rs:208`) **forever** with no max-retries or exponential backoff. On a LAN with many agents that can't reach a coordinator (e.g. coordinator down), the broadcast subnet sees `N_agents × 3s` beacons continuously. The loop never returns on permanent failure, so the outer reconnect loop can't re-resolve config.

**Fix:** Add per-`peer`/`agent_id` cooldown in `serve_discovery` (e.g. `HashMap<SocketAddr, Instant>`); add exponential backoff + max-attempts in `discover_coordinator` (return `Err` after, say, 30 attempts so the outer reconnect loop can re-resolve config).

### M2 — UDP discovery unauthenticated / spoofable `BeaconAck` → agent redirected to a rogue coordinator
**File:** `convwatcher-common/src/discovery.rs:54-93`.

The agent accepts a `BeaconAck` from **any** UDP source, and `serve_discovery` replies to whatever source address sent the beacon. Any host that beats the real coordinator to reply will redirect the agent to an attacker-chosen `tcp_addr:tcp_port`. With the **empty-secret default** (`coordinator.rs:225-228` accepts without auth), the agent will then `Register` against an attacker's fake server, which can issue arbitrary jobs. Even with a non-empty secret, the agent discloses the secret to the attacker's server (it's sent in `Register`, `protocol.rs:64-68`).

**Trigger:** Rogue host on the same L2 segment responding faster than the legit coordinator.

**Fix:** Discovery should not be the trust root — require that the (later) TCP Register secret check gate everything (it does), but also: don't send the secret to a server the agent didn't explicitly configure (when `coordinator_addr` is set explicitly, skip discovery; when discovered, consider pinning the first coordinator that successfully authenticates and refusing re-discovery). Document that empty secret on a multi-tenant LAN is unsafe.

### M3 — Argument-injection via server-controlled codec/quality strings on the agent
**File:** `convwatcher-agent/src/runner.rs:86-145`, `convwatcher-common/src/ffmpeg.rs:25-66`.

`WireVideoRule`/`WireAudioRule` fields (`codec`, `quality`, `audio_codec`, `audio_bitrate`, `output_ext`) are placed verbatim into the ffmpeg argv (`ffmpeg.rs:52-62`, `:90-109`). They are NOT shell-interpolated (good — no shell injection via `Command`), but they ARE inserted as raw argv elements that may begin with `-`, allowing **argument injection**. Example: `codec = "-i"` produces args `... -c:v -i <next_arg> ...` where ffmpeg interprets `-i` as the input flag and the following arg (`quality`/`-c:a`) as the input URL → the agent reads an arbitrary local file and (via `-f <fmt> pipe:1`) streams it back to the server. Combined with M2 / empty-secret default, a rogue coordinator can exfiltrate agent-side files.

Note: AGENTS.md flags custom presets as an RCE surface for *local* overrides, but the same risk exists for *remote* jobs because the agent trusts the coordinator-supplied rule strings.

**Fix:** Validate that `codec`/`quality`/`audio_codec`/`audio_bitrate`/`output_ext` don't start with `-` (or better, allowlist against the preset catalogue). At minimum, prefix-inject any untrusted string with a literal `--`-style terminator — but ffmpeg's argv doesn't support `--`. Allowlisting is the realistic fix.

### M4 — `pick_agent` does not filter `alive`; ties broken by random HashMap order
**File:** `src/worker/coordinator.rs:258-264`.
As in C3, `min_by_key` considers dead agents. Additionally `HashMap` iteration order is randomized → tie-breaking among equally-loaded agents is non-deterministic, which is fine for fairness but makes dispatch order noisy and harder to reason about. Minor selection skew also exists: between `pick_agent` releasing the map lock and `run_on_agent` acquiring `conn.lock()`, another dispatch may pick the same agent; `inflight` reflects this (so subsequent picks are weighted) but the first selection used stale `inflight`. Acceptable under low concurrency; not a correctness bug.
**Fix:** Filter `alive` (resolves the substantive part); optionally use a `BTreeMap` for stable ordering or sort by `(inflight, id)`.

### M5 — `agent_count()` TOCTOU + record-as-failure on transient absence
**File:** `src/worker/dispatch.rs:43-77`, `:228-232`.
`route_job` decides `can_remote` via `pool.agent_count().await > 0` (snapshot), then enters the remote branch. If the (single) agent disconnects in the window between this check and `pool.dispatch`, `dispatch` returns `Ok(false)`, which `run_remote` converts to `Err("no agent available")` → `run_conversion` records it as a **failed conversion** (and per C1 may delete the input). So a *flapping* agent causes video/audio jobs to be recorded as errors (instead of, as documented, processed locally).
**Trigger:** Single agent flaps during the dispatch race window.
**Fix:** Make `Ok(false)` from `dispatch` trigger local fallback (see C1 fix).

### M6 — `Message::Heartbeat`/`JobAbort` defined but never sent → documented protocol features are inert
**Files:** `convwatcher-common/src/protocol.rs:104-108`, `coordinator.rs` (absent), `agent main.rs:275-282`.
Both messages are documented and handled on the agent side, but the server never sends either. Heartbeats would mitigate C3; `JobAbort` would mitigate C4. Their presence suggests intended behavior that was never wired up. Either remove them or implement them.
**Fix:** See C3/C4.

### M7 — Agent `connect_and_serve` reconnect loop can produce unbounded zombie `handle_agent` tasks
**File:** `convwatcher-agent/src/main.rs:189-195`, `coordinator.rs:152-222`.
Agent retries `connect_and_serve` every 3s forever (`main.rs:189-194`). Each successful registration spawns a server-side `handle_agent` task. Combined with C2 (stale handler never exits when sockets die silently), repeated reconnects accumulate dead `handle_agent` tasks and dead `Arc<Agent>`s pinned in the map (until C2 is fixed). Even with C2 fixed, reconnect storms (e.g. agent behind flapping NAT) can briefly double-register. Lower priority than C2 because C2 is the root cause; listed separately because the agent retries aggressively with no jitter/backoff, which amplifies the storm.
**Fix:** Exponential backoff with jitter on the agent reconnect loop; gate on the C2 fix.

---

## 🟢 LOW

| ID | File:line | One-liner |
|----|-----------|-----------|
| L1 | bin/server.rs:228-237 | `local_ip()` returns loopback fallback on air-gapped hosts → advertises `127.0.0.1` agents can't reach. Document or require `--advertise-address`. |
| L2 | transport.rs:120 | `dst.flush().await.ok()` discards flush errors. For a `tokio::fs::File` this is a no-op, but reused for a network `dst` it could hide a real error. |
| L3 | coordinator.rs:320-322 | `create_dir_all(parent).await.ok()` ignored; subsequent `File::create` surfaces a clearer error. Worth a debug log. |
| L4 | runner.rs:229-235,159-178 | Pipe `out_buf`/temp output fully buffered regardless of size → OOM risk on big outputs (cf. H2). Stream instead. |
| L5 | runner.rs:268-274 | `tail()` byte-slice can split a UTF-8 boundary → panic on non-ASCII stderr. Use char-aware slicing or `String::from_utf8_lossy`. |
| L6 | AGENTS.md vs global.rs:255-272 | `worker_io_mode` documented in `GlobalConfig` but actually lives per-agent in `Capabilities`. `WorkerConfig` has only bind/advertise/ports. Doc/code mismatch. |
| L7 | bin/server.rs:114-116 | `Semaphore::new(max_concurrent_conversions as usize)` — if config allows `0`, every spawned job blocks forever. Startup guard needed (same as 05 §C1). |
| L8 | coordinator.rs:224-230 | `subtle::ConstantTimeEq` for `&[u8]` returns early on unequal length. Standard subtle behavior; secret length leaked (low sensitivity). |
| L9 | tests/remote_worker_e2e.rs | Coverage gaps: only `temp` mode exercised (`:104`); `no_agent_falls_back_to_local` tests `WorkerPool::dispatch` only, not `route_job` (C1); `agent_bin()` probing fragile (`:16-31`); hard-coded `/opt/homebrew/bin/ffmpeg` (`:34`) only works on Apple-Silicon macOS; `ffmpeg_ok` guard silently skips on Linux CI. |

---

## Summary

The headline issues are **C1 (fallback contract broken, can delete inputs)**, **C2/C3 (agent pool liveness/registration semantics)**, and **C4 (timeout doesn't kill remote ffmpeg)** — all four contradict explicit statements in AGENTS.md or the modules' own doc comments, and none are caught by the current integration test.