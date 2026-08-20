Vendored copy of [`nucleo`](https://crates.io/crates/nucleo) v0.5.0 (MPL-2.0, see `LICENSE`),
patched to work on `wasm32-wasip1`.

## Why this exists

`nucleo` is the fuzzy matcher behind every `Picker` in helix-term (the file picker, buffer
picker, etc.). `Worker::new` (`src/worker.rs`) unconditionally builds a real
`rayon::ThreadPool` sized to the number of available cores, and `Nucleo::tick_inner`
(`src/lib.rs`) dispatches matching work onto it via `pool.spawn(...)`. `wasm32-wasip1` can't
spawn OS threads at all, so `rayon::ThreadPoolBuilder::build()` always fails there
(`ThreadPoolBuildError` from `std::thread::Builder::spawn` returning
`ErrorKind::Unsupported`), and nucleo `.expect()`s the result - so opening any picker in the
browser build panics the whole wasm instance.

## The patch

1. `src/worker.rs`: split `Worker::new` into a `not(wasm32)` arm (unchanged) and a `wasm32`
   arm. The wasm32 arm doesn't build a per-instance `ThreadPool` at all - instead it registers
   the *calling* thread itself as a rayon worker, exactly once process-wide, via rayon's own
   documented single-thread fallback:
   `rayon::ThreadPoolBuilder::new().num_threads(1).use_current_thread().build()` (the same
   trick `rayon-core`'s default global registry falls back to when it detects thread spawning
   is unsupported - see `rayon_core::registry::default_global_registry`). That registration is
   permanent (rayon intentionally leaks the `WorkerThread` for exactly this use case, so the
   `ThreadPool` handle can be dropped immediately without losing it) - a `std::sync::Once`
   guards against a second `Worker::new` call (opening a second picker) trying to register the
   same thread again, which rayon refuses (`CurrentThreadAlreadyInPool`).
2. `src/lib.rs`: `Nucleo`'s `pool: ThreadPool` field, and the `pool.spawn(...)` call in
   `tick_inner`, are `#[cfg(not(target_arch = "wasm32"))]` - there's no per-instance pool to
   spawn onto on wasm32 (see above), so `tick_inner` runs the matching closure
   (`inner.run(status, cleared)`) inline instead of deferring it. This is necessary, not just
   simpler: a job `.spawn()`-ed into a `use_current_thread()` pool is only ever picked up if
   something explicitly yields to rayon (`yield_now`/`yield_local`/`scope`) on a *different*
   thread, which doesn't exist here.

`rayon`'s actual parallel iterators (`par_iter_mut()` etc., used inside the matching
implementation itself) are unaffected and still un-vendored - they degrade to sequential
execution on a 1-worker pool the normal, well-supported way (via recursive `join()`, executed
directly on the calling thread's own stack), unlike the detached-spawn-and-queue path this
patch avoids.

Diff against the upstream crate is limited to those two files. `Cargo.toml` also drops the
`[workspace]` table and `nucleo-matcher`'s `path = "matcher"` override that only made sense
inside nucleo's own multi-crate repo (this vendors only the single `nucleo` package, not its
`matcher`/`bench` siblings) - `nucleo-matcher` resolves to the normal crates.io dependency.

## How it's wired in

The workspace root `Cargo.toml` has:

```toml
[patch.crates-io]
nucleo = { path = "vendor/nucleo" }
```

which redirects every consumer of `nucleo` to this local copy, for every target - not just
wasm32. Non-wasm32 builds are unaffected since the patch only adds code gated out on other
targets already.

## Upgrading

When bumping to a newer upstream `nucleo` release, re-vendor with
`cp -r ~/.cargo/registry/src/*/nucleo-<version> vendor/nucleo` (minus `Cargo.lock`,
`.cargo-ok`, `.cargo_vcs_info.json`, and swapping in `Cargo.toml.orig` as `Cargo.toml` with the
`[workspace]` table and `matcher` path override removed per above) and reapply the two
wasm32-only arms described above.
