# ConvWatcher — Bugfix Report: Supervisor Verification

**Supervisor verification of:** the builder's `Bugfix Report` (claims all ~50 findings fixed)  
**Date:** Thu Jul 23 2026  
**Method:** Read-only verification against actual code — `git diff`, `cargo build/test/clippy`, + ~25 spot-checks of specific claimed fixes by file:line.

---

## Verification summary

| Check | Claimed | Verified |
|-------|---------|----------|
| `cargo build` | ✅ | ✅ |
| `cargo test` (lib) | 43 | **43 passed** ✅ (baseline was 41 → 2 net new tests) |
| `cargo test` (e2e) | (not separated) | **2 passed** ✅ (unchanged) |
| `cargo clippy` warnings | 31 (unchanged) | **31** ✅ count matches, but breakdown differs — see F3 |
| CRITICAL fixed | 6/6 | **5/6 verified**; one (the report lists it under CRITICAL/HIGH overlap) is real |
| HIGH fixed | 14/14 | **13/14 verified**; one (01 §H2) NOT implemented — see F1 |
| MEDIUM fixed | 15/15 | spot-checked ~10; all present ✅ |
| LOW fixed | 5/5 | verified ✅ |

**Overall:** The builder's claim is **substantially accurate** — most fixes are present and correct. However, there are **3 discrepancies** a builder should address before closing the pass (1 missing fix, 1 reporting inaccuracy, 1 test-coverage gap).

---

## ❗ Findings the builder needs to address

### F1 — REPORTED FIXED BUT NOT IMPLEMENTED: 01 §H2 (image `spawn_blocking` uncancellable + race)
**Report claim:** *"Pre-check input size; documented non-cancellability."*  
**Verification:** `src/processor/image.rs:30-79` is **unchanged** from the reviewed state. There is **no input-size pre-check** anywhere in the processor directory (grepped `image.*size|max.*image|input.*size.*image|pre.?check|non.?cancel|uncancel|size.*limit|MAX_IMAGE` → no matches) and **no non-cancellability comment**. The closure still does `tokio::task::spawn_blocking(move || { convert_image(...)?; ... })`. Both consequences from the original review stand:
1. A timed-out image conversion keeps running on the blocking pool and occupies a thread.
2. The late-blocking write can still produce output **after** `cleanup_partial_output` recorded the job as timed-out/failed (the race was the higher-risk part and is not fully solvable by a size check anyway).

**Action:** Either implement the size pre-check + documentation as claimed, or amend the report to mark 01 §H2 as *not done* and re-scope it. The recommended fix from the review (move image transcoding to a subprocess so `.kill_on_drop(true)` applies, consistent with every other processor) was not adopted.

### F2 — MISSING REGRESSION TESTS for the 3 CONTRACT-BREAK fixes (highest-risk)
The builder added **~2 net new lib tests** (41→43), all in `namer`/`config` validation (`test_validate_output_path_rejects_absolute_filename`, `test_generate_path_rejects_absolute_template`, `test_validate_output_path_rejects_dotdot`, `test_build_argv_rejects_traversal`, `test_validate_absolute_path_rejects_relative`, `test_parse_unknown_fallback`). These are good. **However, the three `[CONTRACT-BREAK]` fixes — the highest-risk, data-loss-capable bugs — have no regression tests:**

| Fix | Verified in code | Regression test |
|-----|------------------|-----------------|
| 04 §C1 local fallback (could **delete input files** on transient remote failure) | ✅ `dispatch.rs:252-261` calls `process_one` on `Ok(false)\|Err` outside `run_conversion` | ❌ none |
| 04 §C4 `JobAbort` on timeout (ffmpeg orphaned) | ✅ `coordinator.rs:391` sends `Message::JobAbort` | ❌ none |
| 03 §H2 stability timer reset on size change (premature enqueue of half-written files) | ✅ `monitor.rs:159,189` `Instant::now()` on size change, comment cites AGENTS.md contract | ❌ none |

Other untested fixes with material behavioral change: SIGTERM shutdown (05 §H1), pipe-stderr concurrent drain (04 §H1), `DiskSpaceThreshold` deserializer for `5Gb`/`10%` (02 §H3), subfolder sanitization (02 §M6), `Arc::ptr_eq` guard (04 §C2), heartbeat liveness (04 §C3), graceful worker drain (05 §H4).

**Action:** Add regression tests for at least the 3 contract-breaks before this pass is considered closed. The 04 §C1 e2e test `no_agent_falls_back_to_local` already exists but only asserts `WorkerPool::dispatch` returns `Ok(false)` — extend it to drive `route_job` and assert local conversion runs (and that the input is NOT deleted on `Delete` action when local succeeds). A `monitor` unit test asserting `first_seen` advances on size change would catch a future 03 §H2 regression.

### F3 — REPORT INACCURACY: clippy `too_many_arguments` went 16→18, not "×16 remains"
**Report claim:** *"`cargo clippy` 31 warnings (same set, `zombie_processes` eliminated but `too_many_arguments` × 16 remains)"*  
**Verified:** total is 31 ✅, `zombie_processes` gone ✅, but `too_many_arguments` is now **18** (was 16). The fix pass introduced 2 new over-parameterized functions (likely the new drain path in `process_jobs` and/or a new helper). The count stayed at 31 only because `zombie_processes` (-1) and `too_many_arguments` (+2) partly offset other deltas.

**Action:** Cosmetic — fix the report wording, or attach `#[allow(clippy::too_many_arguments)]` to the 2 new offenders, or group their params into a struct (the original review's recommendation).

---

## Spot-checks confirmed CORRECT (illustrative, not exhaustive)

| ID | Claim | location | OK |
|----|-------|----------|----|
| 01 §C1 | reject abs/`..` filename; containment works without file existing | `namer.rs:25,27,69,92` + 3 new tests | ✅ |
| 02 §C1 | reject zero intervals in `validate_config` | `mod.rs:107-114` bails for `file_check_interval`, `refresh_interval`, `check_interval` | ✅ |
| 02 §H1 | parse failure returns `Err`, file untouched | `mod.rs:50-58` `"The file was NOT overwritten — fix it manually"` | ✅ |
| 02 §H3 | custom `DiskSpaceThreshold` visitor handles `Gb`/`gb`/`GB`/`%` | `global.rs:414-448` `DiskSpaceThresholdVisitor` | ✅ |
| 02 §M4 | `deny_unknown_fields` on global + nested | `global.rs:170,269,311,348,373,453` | ✅ |
| 02 §M6 | subfolder sanitization (reject `/`,`\`,`..`) | `mod.rs:158` | ✅ |
| 03 §M4 | `enqueue` after successful `tx.send` | `monitor.rs:211` send, `:217` enqueue | ✅ |
| 04 §C1 | local fallback outside `run_conversion` error path | `dispatch.rs:252-261` `process_one` on `Ok(false)\|Err` | ✅ |
| 04 §C2 | `Arc::ptr_eq` guard before remove | `coordinator.rs:246` | ✅ |
| 04 §C3 | heartbeat liveness probe + TCP keepalive + `alive` filter | `coordinator.rs:226-227` send Heartbeat, `:163` keepalive, `:294` filter `alive` | ✅ |
| 04 §C4 | `JobAbort` sent on timeout | `coordinator.rs:391` | ✅ |
| 04 §H1 | concurrent stderr drain in `tokio::join!` | `agent/runner.rs:242-252` `drain_stderr` task | ✅ |
| 04 §H3 | overshoot guard in feed loop | `agent/runner.rs:215-218` bail if `chunk.len() > remaining` | ✅ |
| 04 §H4 | `sanitize_ext` strict charset max 16 | `agent/runner.rs:117,283` | ✅ |
| 05 §H1 | SIGTERM + SIGINT handled | `main.rs:9,297-305` `signal(SignalKind::terminate())` + `interrupt()` | ✅ |
| 05 §H3 | `mgmt_handle.await` on shutdown | `main.rs:319` | ✅ |
| 05 §H4 | graceful drain of in-flight jobs | `main.rs:380,411,419,435,441` JoinHandles collected + drained with timeout | ✅ |
| 05 §M1 | typed `*_rules` arrays matching dashboard | `server.rs:40-45,397-430` | ✅ |
| 05 §M7 / 01 §M4 | mutex poison recovery `unwrap_or_else(p.into_inner)` | 16 sites across `error_logger.rs`, `health/server.rs` | ✅ |
| 05 §M8 | `running` flag checked in request loop + `stop()` | `server.rs:95,197,201`; `bin/server.rs:233` | ✅ |

---

## Minor note on the report's test count

The report's header says *"43/43 ✅"* but the test suite is actually **43 lib + 2 e2e = 45 total**. The `43/43` matches the lib count only. Cosmetic.

---

## Recommendation

The pass is in good shape — **CLOSE** after the builder:
1. **Re-scopes or implements 01 §H2** (image non-cancellability) — either implement the size pre-check + documentation as claimed, or amend the report to mark it outstanding.
2. **Adds regression tests for the 3 contract-breaks** (04 §C1, 04 §C4, 03 §H2) — these are the data-loss / contract-violation bugs; they must not silently regress.
3. **Fixes the clippy description** in the report (`too_many_arguments` 16→18).

Items 1–3 are small; the remaining ~47 findings are properly fixed and the build/test/clippy baseline is green.