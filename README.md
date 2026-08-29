# Ferrite

A memory-safe, Rust-native reimplementation of the Cheat Engine idea: attach
to a running process, scan its memory for values, filter down through
repeated scans, and edit or freeze what you find.

This is a clean-room build — not a port of Cheat Engine's own codebase — built
because the official Windows installer wraps its real payload in a
third-party bundler signed under an unrelated corporate identity and
reproducibly crashes on install. Ferrite exists to be auditable, memory-safe
by construction, and maintained in the open.

## Status

**v1 is feature-complete.** The full core loop works and is verified
against a real target process: **attach → scan → filter (next scan) →
edit / freeze → save & load a cheat table** (our own JSON format, plus
importing existing Cheat Engine `.CT` tables).

- Process attach/detach, with elevation errors surfaced as readable text.
- Exact-value scanning (`i8`–`i64`, `f32`/`f64`) and byte-pattern (AOB)
  scanning, both with next-scan filters (changed/unchanged/increased/
  decreased).
- Live-refreshing results table, writing a new value to selected results,
  and freeze/unfreeze (a background thread pins a value against whatever
  the target does to it).
- A saved list separate from scan results: promote a result, or add an
  address manually (module-relative or absolute, with an optional
  single-level pointer offset).
- Save/load your own cheat table as plain JSON, or import a real Cheat
  Engine `.CT` file — unsupported entries (Lua scripts, multi-level
  pointer chains, etc.) are reported visibly, never silently dropped or
  guessed at.

Known limitation: v1 is Windows-only, 64-bit targets only. See the
Roadmap section for what's next (post-v1 stretch goals).

## Workspace layout

- [`ferrite-core`](ferrite-core) — GUI-free core: process attach, memory
  scanning, read/write primitives, module/pointer resolution, cheat-table
  persistence and `.CT` import. Unit- and integration-tested independent
  of the GUI (including against a small helper process, `ferrite-victim`).
- [`ferrite-gui`](ferrite-gui) — the desktop application
  ([`egui`](https://github.com/emilk/egui)/`eframe`), consuming
  `ferrite-core`.
- [`ferrite-victim`](ferrite-victim) — a tiny test-only helper process with
  known values at known addresses, used by `ferrite-core`'s integration
  tests. Not shipped.

## Building

```sh
cargo build --workspace
cargo test --workspace
cargo run -p ferrite-gui
```

Windows only, 64-bit targets — `ferrite-core` uses Win32 process/memory
APIs via [`windows-rs`](https://github.com/microsoft/windows-rs).

## Roadmap

v1's core loop (above) is done. Next up is a stretch milestone,
post-v1: AOB scan as a search (not just a compare), multi-level pointer
chains, an unknown-initial-value scan, string value types, and 32-bit
target support. Full scope, architecture, and milestone plan live in the
project's planning vault (maintained alongside this repository).

## License

Licensed under either of

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
