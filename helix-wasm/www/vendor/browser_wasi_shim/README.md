Vendored from [`@bjorn3/browser_wasi_shim`](https://github.com/bjorn3/browser_wasi_shim)
v0.4.2 (MIT OR Apache-2.0, see `LICENSE-MIT`), fetched from the package's published
`dist/` output on unpkg since the upstream repo doesn't commit its compiled output.

Only the modules `helix-wasm/www/index.js` actually imports are vendored:
`wasi_defs.js`, `debug.js`, `fd.js`, `fs_mem.js` (for `ConsoleStdout`), and `wasi.js`
(the `WASI` class). The package's own `index.js`, `fs_opfs.js` (OPFS-backed files) and
`strace.js` are intentionally not included since they aren't used here — files are
copied verbatim, not modified, other than that.
