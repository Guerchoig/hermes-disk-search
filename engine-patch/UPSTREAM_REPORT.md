# Upstream report: OpenResearchtools/engine v1.15 — cluster API lock bugs, unbounded slot wait, model load under lock, and missing KV cache type

**Component:** `bridge/` — the cluster + bridge C API of the engine (`llama_server_cluster.*`,
`llama_server_bridge.*`).
**Version:** tag `v1.15` (commit `2683eb6`); vendored llama.cpp (`ggml 0.9.7`).
**Host where found:** Windows 11 x64, NVIDIA RTX 3060 12 GB, CUDA 13.4, MSVC (VS 18),
CMake 4.3, Ninja Multi-Config.

## Summary

We embed the engine (the cluster + bridge C API) inside a persistent host process that owns the
GPU and serves chat / embeddings / rerank / ASR on demand, reusing the same DLLs. During that
integration we hit an incident where **a single hung engine call froze the whole engine**: the
engine's status (`list_instances`, `/internal/status`), eviction (`unload_instance`),
retention changes and *every other role* all blocked on the same mutex, and the ~10 GB of VRAM
held by the hung call could not be released — not even by `unload`. Root-cause analysis found
**three concurrency defects** plus **one API gap**. We carry a small patch (four files,
additive and ABI-safe) until these are addressed upstream; this report is its rationale, with
reproduction and suggested fixes. We are happy to send it as a PR.

## 1. Unbounded wait for an instance slot (availability hang)

`wait_for_instance_slot_locked()` (in `llama_server_cluster.cpp`) blocks a request while
`instance.active_request_count >= instance_parallel_limit(instance)` with a bare
`instance.cv.wait(lock);` inside a `while` loop. There is **no deadline anywhere in the engine**
(`wait_for` / `wait_until` do not appear in the file). A request whose slot notification is
lost — or that races a stuck call — therefore **waits forever**, and because the caller holds
the instance request lock for the whole wait, the role becomes permanently unavailable.

**Suggested fix.** Wait with a deadline, e.g. `instance.cv.wait_until(lock, now + 600s)`; on
timeout record it in `instance.last_error` (so it surfaces through `list_instances` → host
status) and proceed via the queue instead of hanging.

## 2. Model load executed under `model_instance::mutex`

`ensure_instance_loaded_locked()` performs the actual model load (seconds of work) **while the
caller holds `instance->mutex`**. During that window `llama_server_cluster_list_instances()`,
`llama_server_cluster_unload_instance()`, `llama_server_cluster_set_instance_retention_mode()`
and every other request on that instance block on the same mutex. In our incident this is
precisely what turned one slow/hung load into a total freeze of the engine.

**Suggested fix.** Do the load **outside** the instance mutex: build the bridge from a detached
helper that reads only immutable `params`, serialize concurrent loads with a dedicated
`load_mutex`, and have `ensure_instance_loaded()` take/release the request lock in short
critical sections (publish the bridge + `cv.notify_all()` under the lock only).

## 3. Lock-order inversion (ABBA): `set_cluster_error()` called while holding `instance->mutex`

`set_cluster_error()` takes `cluster->mutex`. `llama_server_cluster_remove_instance()` (and
`find_instance_by_name()`) take `cluster->mutex` **then** `instance->mutex` — order
**cluster→instance**. Several paths took the locks in the *opposite* order, i.e. holding
`instance->mutex` while calling `set_cluster_error()` (**instance→cluster**):

* `llama_server_cluster_unload_instance()` — the "instance is busy" path;
* `llama_server_cluster_set_instance_retention_mode()`;
* the request paths' tails in `chat_complete`, `vlm_complete`, `embeddings`, `rerank`,
  `audio_transcriptions_raw` (load-failure branch and the final error/success `set_cluster_error`); and
* the `enable_diarization` guard in `llama_server_cluster_audio_transcriptions_raw()`.

Any of these can deadlock against a concurrent `remove_instance()`.

**Suggested fix.** Call `set_cluster_error()` only with the instance lock released (do not
acquire any new lock while holding the instance mutex). The request tails are already
lock-free in v1.15 (`finish_request_locked(...); lock.unlock();` precedes the tail); the
remaining offenders are the busy/retention/load-failure/guard paths.

## 4. No KV cache type in the cluster/bridge API → `--cache-type-k/v` silently ignored

`llama_server_cluster_instance_params` and `llama_server_bridge_params` have **no fields for
the KV cache type** (neither the structs nor the implementation mention `cache_type`).
Therefore the cluster/bridge API cannot express a quantized KV cache even though the vendored
llama.cpp supports it end-to-end:
`common_params.cache_type_k/v` (`common/common.h`) → `llama_context_params.type_k/type_v`
(`include/llama.h`). The engine consequently always allocates KV as **F16** — for our
16k-context chat model that is ≈512 MiB — and there is no way to request `q8_0` through the API.

**Suggested fix.** Add `int32_t cache_type_k, cache_type_v` to both structs (appended at the
end for ABI safety), propagate them into `bridge->params.cache_type_k/v`, and enable flash
attention when the V cache is quantized (llama.cpp requires FA for `type_v != F16`).

## Reproduction

* **[1]** Fill all `n_parallel` slots of an instance (or drop a slot wakeup), then issue one more
  request: it blocks in `wait_for_instance_slot_locked()` indefinitely.
* **[2]** Start a request that triggers a model load, and concurrently call
  `llama_server_cluster_list_instances()` / `unload_instance()`: both block for the whole load.
* **[3]** From one thread loop `remove_instance()` while another thread runs `unload_instance()`
  (busy path), `set_instance_retention_mode()`, or a failing request path — ABBA against the
  cluster→instance order.
* **[4]** Set `--cache-type-k q8_0 --cache-type-v q8_0` in the server config and measure the KV
  allocation (or the VRAM delta as `n_ctx` grows): it stays F16, because the value never reaches
  the bridge.

## Impact

* **Availability:** a single stuck/slow engine call can freeze status, eviction and all roles, and
  pin VRAM that `unload` cannot free. On Windows/WDDM the holding process's VRAM is not visible
  in `nvidia-smi` (reported as `N/A`), which makes the freeze hard to diagnose.
* **Memory:** the missing KV cache type blocks a supported ~2× KV reduction for long-context
  models (our chat: F16 ≈512 MiB → `q8_0` ≈272 MiB at `n_ctx 16384`).

## Our patch (reference) and measurements

`hds-engine-patch-v1.15.patch` — 4 files, additive/safe; **ABI unchanged** (exports of
`llama-server-bridge.dll` = 113, `multi-node-server.dll` = 39, before and after). Base `v1.15`.
It implements the four fixes above: (P1) bounded slot wait, (P2) detached load with `load_mutex`,
(P3) `set_cluster_error` outside the instance lock, and (KV) the two `cache_type_*` fields +
flash attention. Our Rust host is compatible with **both** stock and patched DLLs (the new struct
fields are appended and written explicitly, so a stock DLL simply ignores them).

Verified on a copy of the runtime (host resident untouched): the chat model loads with flash
attention auto-enabled; measured KV ≈ **1.68 KiB/token/layer** vs the F16 formula's 4.00 KiB
(−58%), i.e. `q8_0`; a chat completion returns correctly.

## Provenance / how to reproduce the build

See `engine-patch/README.md`: clone at tag `v1.15`, apply the patch to `bridge/`, build with
`vcvarsall x64` + **Ninja Multi-Config**, `-Backend cuda -EnableBackendDl -DisableGgmlNative`.
Patch overlay for our installers: release tag `engine-patch-v1`
(`installers/fetch_engine_runtime.ps1 -PatchEngine`; rollback `-RollbackEnginePatch`).

> Note: this patch is a temporary overlay on our side. If/when these issues are fixed upstream we
> drop it and switch back to the stock engine.
