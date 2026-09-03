# Changelog

All notable changes to Ferrite are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pre-1.0, a minor version bump may carry a breaking change — see `0.2.0`.

## [Unreleased]

Nothing yet.

## [0.2.0] — 2026-09-04

[Release](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v0.2.0)

### Changed — BREAKING

- `CheatEntry.pointer_offset` (a single optional offset) became
  `pointer_offsets` (a list). **Cheat tables saved by `0.1.0` no longer
  load**, failing with ``missing field `pointer_offsets` `` — loudly,
  naming the field, rather than importing something wrong. To migrate a
  saved table by hand, rename the field and wrap the value in a list:
  `"pointer_offset": null` → `"pointer_offsets": []`, and
  `"pointer_offset": 8` → `"pointer_offsets": [8]`.

  Pre-1.0, so this is a deliberate clean break rather than a compatibility
  shim for a format with no external users. Cheat Engine `.CT` files are
  unaffected — they're imported, not migrated.

### Added

- **String value types.** `String` (Latin-1) and `Unicode String`
  (UTF-16LE) join the scan-type list, matching Cheat Engine's own names.
  A string scan is a byte-pattern search underneath, so it reuses the
  existing AOB engine and its next-scan filters; results display as
  decoded text rather than hex.
- **Multi-level pointer chains.** A saved entry holds a chain of offsets
  instead of one. The manual-add form takes them as comma- or
  space-separated hex values in a single field, and `.CT` entries with two
  or more `<Offset>` elements import properly instead of being skipped.
  Chains are capped at 16 levels on input so a malformed table can't drive
  an arbitrarily long sequence of reads on every refresh tick.
- **Script text for entries Ferrite can't run.** A skipped
  `Auto Assembler Script` entry carries its raw `<AssemblerScript>` text
  into the import report, behind a collapsed "Show script" section.
  Ferrite never executes it — the point is that a downloaded table's
  script can be *read* before it's trusted.
- `.CT` import handles `<Length>`, `<Unicode>` and `<ZeroTerminate>` for
  string entries.

### Notes on Cheat Engine compatibility

These were read out of Cheat Engine's own source rather than inferred,
because each fails silently if guessed wrong:

- `<Length>` is a **character** count, not a byte count — a unicode
  entry's buffer is twice its declared length. Parsed as decimal, unlike
  the hex `<Offset>` beside it.
- An absent `<ZeroTerminate>` element means **true**, not false: CE's
  `setVarType` turns it on for either string type before the XML read path
  can override it.
- Pointer-chain offsets are stored in **document order** and walked
  **last to first** — N offsets means exactly N dereferences, with no
  trailing one. An empty list is a direct address, not one dereference of
  offset zero.
- `<CodePage>1</CodePage>` string entries are reported as unsupported
  rather than decoded as Latin-1, which they aren't.

One deliberate divergence: CE's *write* path also consults
`ZeroTerminate` (it appends a terminator). Ferrite always writes the full
fixed-width buffer, so `ZeroTerminate` here affects display only.

## [0.1.0] — 2026-08-30

[Release](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v0.1.0)

First tagged release. The full core loop, verified against a real target
process: **attach → scan → filter (next scan) → edit / freeze → save &
load a cheat table**.

### Added

- Process attach/detach, with elevation errors surfaced as readable text.
- Exact-value scanning (`i8`–`i64`, `f32`/`f64`) and byte-pattern (AOB)
  scanning, both with next-scan filters
  (changed/unchanged/increased/decreased).
- Live-refreshing results table, writing a new value to selected results,
  and freeze/unfreeze (a background thread pins a value against whatever
  the target does to it).
- A saved list separate from scan results: promote a result, or add an
  address manually — module-relative or absolute, with an optional
  single-level pointer offset.
- Save/load a cheat table as plain JSON, and import a real Cheat Engine
  `.CT` file. Entries Ferrite can't represent are reported visibly, never
  silently dropped or guessed at.
- Process-list icons, native file dialogs, a dark theme, and the
  application's own icon.

[Unreleased]: https://github.com/F3rNaNDEZ57/Ferrite/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/F3rNaNDEZ57/Ferrite/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v0.1.0
