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

**v0.2.0 is out.** The full core loop works and is verified against a real
target process: **attach → scan → filter (next scan) → edit / freeze →
save & load a cheat table** (our own JSON format, plus importing existing
Cheat Engine `.CT` tables).

- Process attach/detach, with elevation errors surfaced as readable text.
- Exact-value scanning (`i8`–`i64`, `f32`/`f64`), byte-pattern (AOB)
  scanning, and string scanning (`String` / `Unicode String`, i.e.
  Latin-1 and UTF-16LE) — all with next-scan filters
  (changed/unchanged/increased/decreased).
- Live-refreshing results table, writing a new value to selected results,
  and freeze/unfreeze (a background thread pins a value against whatever
  the target does to it).
- A saved list separate from scan results: promote a result, or add an
  address manually (module-relative or absolute, with an optional
  multi-level pointer chain).
- Save/load your own cheat table as plain JSON, or import a real Cheat
  Engine `.CT` file. Multi-level pointer entries and string entries import
  properly; entries Ferrite still can't represent are reported visibly,
  never silently dropped or guessed at — and a skipped Auto Assembler /
  Lua entry now shows you its script text, so you can read what it would
  have done before deciding to trust it. Ferrite never executes it.

Known limitation: Windows-only, 64-bit targets only. See the Roadmap
section for what's still deferred.

Release notes for each version are in [`CHANGELOG.md`](CHANGELOG.md).

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

The core loop (above) is done, and v0.2.0 added string value types,
multi-level pointer chains, and script-text display for entries Ferrite
can't run. Still deferred, each needing its own design pass first:

- **An unknown-initial-value scan** — needs a real storage decision
  (on-disk / mmap / capped-in-RAM snapshot of potentially GB-scale
  memory), not a small addition.
- **32-bit target support** — touches pointer-width assumptions across
  already-shipped, tested code.
- **"AOB scan as a search, not just a compare"** — carried over from the
  original stretch list, but its intent was never pinned down (the
  current AOB scan already does an unaligned substring search); it needs
  clarifying before it can be scoped.

Explicitly *not* planned: running Lua / Auto Assembler scripts. That
needs an embedded interpreter plus a large reimplementation of Cheat
Engine's own Lua API — and, for plain Auto Assembler scripts, an x86-64
assembler and code-injection machinery. Ferrite only ever touches data,
never patches code or redirects execution.

Full scope, architecture, and milestone plan live in the project's
planning vault (maintained alongside this repository).

## License

Licensed under either of

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
