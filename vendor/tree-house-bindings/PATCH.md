Vendored copy of [`tree-house-bindings`](https://crates.io/crates/tree-house-bindings)
v0.3.2 (MPL-2.0, see `LICENSE`), patched to build on `wasm32-wasip1`.

## Why this exists

`helix-loader` statically links tree-sitter grammars on `wasm32-wasip1` via this crate's
`tree-sitter-language` feature (`Grammar: TryFrom<LanguageFn>`) instead of the normal
dlopen-based `Grammar::new`, since there's no runtime dynamic linking in a browser. But the
crate as published unconditionally imports `libloading::{Library, Symbol}` in
`src/grammar.rs` regardless of features - `libloading`'s `Library`/`Symbol` types are gated
to `cfg(any(unix, windows, libloading_docs))`, none of which match `wasm32-wasip1`. So the
crate fails to compile for this target at all, independent of which features are enabled.

## The patch

1. `src/grammar.rs`: gate the `libloading` import and `Grammar::new` (the dlopen-based
   loader) behind `#[cfg(not(target_arch = "wasm32"))]`.
2. `src/query_cursor.rs`: `ts_query_cursor_set_byte_range`'s `extern "C"` declaration was
   missing the `-> bool` return type the actual C function
   (`vendor/include/tree_sitter/api.h`) has. This is a genuine, independent upstream bug -
   harmless on native ABIs (the extra return value is just discarded at both call sites,
   which already ignore it), but wasm's strict function-type checking on direct calls traps
   on the mismatch at link time (`signature_mismatch:ts_query_cursor_set_byte_range`),
   surfaced only by actually running the wasm32-wasip1 build. Fixed by adding the missing
   `-> bool`.

Diff against the upstream crate is limited to those two files.

## How it's wired in

The workspace root `Cargo.toml` has:

```toml
[patch.crates-io]
tree-house-bindings = { path = "vendor/tree-house-bindings" }
```

which redirects every consumer of `tree-house-bindings` (both `tree-house` itself and
`helix-loader`'s direct wasm32-only dependency, see `helix-loader/Cargo.toml`) to this local
copy, for every target - not just wasm32. Non-wasm32 builds are unaffected since the patch
only removes code gated out on other targets already.

## Upgrading

When bumping to a newer upstream `tree-house-bindings` release, re-vendor with the same
`cp -r ~/.cargo/registry/src/*/tree-house-bindings-<version> vendor/tree-house-bindings`
(minus `Cargo.lock`/`Cargo.toml.orig`/`.cargo-ok`/`.cargo_vcs_info.json`) and reapply the two
`#[cfg(not(target_arch = "wasm32"))]` gates described above.
