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

**Pre-alpha. Not yet functional.** This repository currently contains only
the initial workspace skeleton — no scanning, attach, or GUI logic exists
yet. See the Roadmap section below for what's planned first.

## Workspace layout

- [`ferrite-core`](ferrite-core) — GUI-free core: process attach, memory
  scanning, read/write primitives, pointer resolution. Built to stay
  unit-testable independent of the GUI.
- [`ferrite-gui`](ferrite-gui) — the desktop application
  ([`egui`](https://github.com/emilk/egui)/`eframe`), consuming
  `ferrite-core`.

## Building

```sh
cargo build
cargo test
cargo run -p ferrite-gui
```

Windows only for now — `ferrite-core` will depend on Win32 process/memory
APIs via [`windows-rs`](https://github.com/microsoft/windows-rs).

## Roadmap

v1 targets one core loop: **attach → scan → filter (next scan) → edit /
freeze → save & load a cheat table** (our own format, plus import of existing
Cheat Engine `.CT` tables). Explicitly out of scope for v1: a disassembler /
debugger view, code injection, scripting, kernel-driver anti-anti-cheat,
structure dissect, and platform inspectors (.NET/Java/Mono). Full scope,
architecture, and milestone plan live in the project's planning vault
(maintained alongside this repository).

## License

Licensed under either of

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
