# Changelog

All notable changes to Ferrite are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pre-1.0, a minor version bump may carry a breaking change — see `0.2.0`.

## [Unreleased]

Nothing yet.

## [1.1.1] — 2026-09-04

[Release](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v1.1.1)

### Fixed

- **A table's own `<LuaScript>` is now shown.** That element belongs to no
  entry, and Ferrite wasn't reading it at all — so it never appeared in the
  import report.

  It matters because **Cheat Engine runs `<LuaScript>` the moment a table
  is opened**: no entry enabled, nothing clicked. It is simultaneously the
  most security-relevant part of a downloaded table and the part you are
  least likely to know is there, and Ferrite was hiding it from the one
  screen built to show you what a table would do.

  Ferrite still never runs it, and — unlike an entry's script — does not
  offer to. An auto-run script presented as something you opt into would
  misrepresent what the table was built to do.

Found by importing a real downloaded `.CT` rather than a test fixture.
Every script fixture written for 1.1.0 exercised the classifier, and none
carried a `<LuaScript>`.

## [1.1.0] — 2026-09-04

[Release](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v1.1.0)

**Ferrite can now run the data-only Lua scripts in a Cheat Engine table.**
Nothing else changes: saved tables load unchanged, and every existing
feature behaves as it did.

### The important distinction

"Run the Lua cheats in a `.CT` file" sounds like one feature. It is two,
and only one of them is here.

- A **`{$LUA}` script** runs *in Ferrite's own process* and acts on the
  target through ordinary reads and writes — which is what Ferrite already
  does for every scan. This release runs those.
- An **Auto Assembler script** allocates memory *inside the target*, writes
  machine code into it, and patches the target's execution to run that
  code. Ferrite still does none of this, and it is still not planned.

**Most god-mode cheats in downloaded tables are the second kind.** The
import report now labels every script entry with which it is, so you can
tell before trying.

### Added

- **`{$LUA}` script execution**, enabled and disabled per entry from the
  import report. `[ENABLE]` and `[DISABLE]` work as they do in Cheat
  Engine, and `[DISABLE]` runs automatically when you detach — a script
  that changed values gets its restore path while there is still a process
  to run it against.
- **A data-only API**: `readInteger` / `writeInteger` and the rest of the
  read, write, address-resolution and module functions, with Cheat
  Engine's own signatures. An address argument may be a number or a
  `module.exe+offset` string, as CE allows.
- **Script classification** in the import report — data-only Lua, generates
  code, Auto Assembler, no code, or unreadable — so an entry Ferrite can
  run is visibly different from one it can only show you.
- **A consent step.** No script runs on import, or without being agreed to
  once per entry per session, with the script readable beside the prompt.

### The sandbox, and its limits

Scripts run in a Lua interpreter where the dangerous operations **do not
exist**, rather than being inspected and judged safe. `io`, `os`,
`package`, `require` and `debug` are never loaded; `load`, `loadstring`,
`dofile` and `loadfile` are removed after construction, because they ship
in Lua's base library and `dofile` reads and executes a file from disk.
None of Cheat Engine's `autoAssemble`, `executeCode`, `allocateMemory`,
`injectDLL` or debugger functions is provided, and there are no no-op
stubs — a script reaching for one fails on a nil value rather than
reporting success having done nothing.

Runs are bounded three ways: 10 million VM instructions, 64 MB of Lua
memory, and 2000 lines of retained output. The memory limit matters more
than it sounds: `string.rep('x', 400000000)` is a *single* VM instruction,
so the instruction budget cannot see it — measured allocating 400 MB in
640 ms before the limit existed.

**What the sandbox does not do:** a script can write to any address in the
attached process. That is what a cheat *is*, so it cannot be designed
away. "Sandboxed" here means it cannot reach your filesystem, your
network, or the target's execution — it does not mean the script is
harmless, which is why running one is a deliberate act.

### Notes

- **The security review of this release was self-conducted.** The intended
  independent review pass could not be obtained. The sandbox was instead
  tested adversarially — the memory limit above exists because that testing
  found the hole — but that testing was done by the same author as the
  design, which is the blind spot an independent reading exists to cover.
  Weigh that accordingly before running scripts you did not write.
- A failed enable that had already started reports as **partly applied**
  rather than on or off, since neither would be true. Such an entry keeps a
  "Run disable anyway" button.
- Script state is per session, and scripts are not saved into Ferrite's own
  table format — that format carries no executable content, so a script
  there would be silently dropped on save.
- `ferrite-core` gained `script`, `lua` and `lua_api` modules. It is
  workspace-internal rather than a published crate, so this is not a
  public API commitment.

## [1.0.0] — 2026-09-04

[Release](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v1.0.0)

The interface, rebuilt. Cheat tables saved by `0.3.0` load unchanged —
this release changes how Ferrite looks and how its window is arranged,
not what it stores.

### Changed — the whole interface

- **Three docked regions replace five stacked cards.** A top bar, a left
  rail holding every scan control, a central results region, and a
  saved-list dock. Each region has its own scroll, so the saved list can no
  longer be pushed off the bottom of the window by anything above it.
- **The results table is virtualised** at a fixed 24 px row, with exact
  column widths. A scan can return tens of thousands of addresses and every
  visible row re-reads target memory ten times a second, so only the rows
  actually on screen are built at all.
- **Addresses line up.** They are printed as a fixed-width 16 digits with
  the leading zeros dimmed, instead of `0x14a20` sitting next to
  `0x7ff6a41c58da` and reading as ragged text rather than a column.
- **Every fallible field owns a fixed message slot**, so an error can never
  move the layout, and the scan field validates as you type — First Scan
  stays disabled until the value parses, which makes an invalid scan
  impossible rather than merely reported.
- **A new palette and type scale.** A warm near-black ground and a single
  oxide-red accent that means exactly one thing: attention. A red fill is an
  action you may take; red type or a red rule is a problem you must read.
  Zero corner radius everywhere, no shadows, and Archivo + JetBrains Mono
  embedded in the binary (both OFL) so it renders the same on any machine.
- **Columns drop rather than squeeze** as the window narrows, and the layout
  holds down to 1024 × 700. The rail never collapses — it holds the primary
  action.

### Added

- **A process filter and an architecture column.** The picker filters over
  name, PID and path, and shows which targets are 32-bit — those say
  "64-bit only" instead of offering an Attach that would fail later. Hiding
  them is on by default.
- **A `PREVIOUS` column** showing what each address held before the last
  rescan, so a filtered set shows what it was filtered against.
- **A `MODULE + OFFSET` column**, resolving each address back to
  `game.exe+1C58DA0` where it falls inside a loaded module.
- **The import report is a split view**: skipped entries on the left, the
  selected entry's script in full on the right, with Copy and a wrap toggle.
  It exists so a downloaded table's script can be *read* before it is
  trusted. Ferrite still never assembles, injects or runs any of it.
- **Manual add is a validating modal** that shows the pointer expression it
  is building — `[[7FF698F52228]+0]+0` — before anything is added, and keeps
  Add disabled until the whole form parses.
- **A live value flashes** when it changes on the refresh tick and decays
  back over 400 ms, so a change is visible even if you were looking
  elsewhere.
- Scan history, as the chain of match counts: `18402 → 412 → 6`.

### Notes

- `ferrite-core` gained `ScanMatch::previous`, `AobMatch::previous`,
  `ProcessInfo::arch` and `ModuleMap`. It is a workspace-internal library,
  not published, so this is not a public API break — but a `ModuleMap` is
  deliberately a *snapshot*: it goes stale when the target loads a DLL, and
  resolves an address in a newly-loaded module to nothing rather than to
  something wrong.
- Still Windows-only, 64-bit targets only, single executable, nothing
  written outside its own folder, no network, no telemetry.

## [0.3.0] — 2026-09-04

[Release](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v0.3.0)

No breaking changes — tables saved by `0.2.0` load unchanged.

### Added

- **`.CT` import: `Pointer` entries.** A pointer isn't a distinct shape —
  Cheat Engine's own `setVarType` rewrites `vtPointer` to `vtQword` /
  `vtDword` and turns `ShowAsHex` on. It's an address-sized integer shown
  in hex, always the 8-byte form here since Ferrite is 64-bit only.
- **`.CT` import: `Array of byte` entries**, sized by `<ByteLength>`
  (decimal). Imported as the same byte-buffer shape an AOB scan produces,
  so the value displays as a hex pattern that pastes straight back into
  the AOB search box.
- **`<ShowAsHex>` support**, for any numeric entry rather than only
  pointers. `CheatEntry` gains `show_as_hex`, additive and defaulted, so
  no saved table needs migrating. Display only — a saved entry's value is
  rendered, never parsed back from the row, so there's no ambiguity about
  which base typed input would be in. Floats render their bit pattern,
  matching CE's treatment of a hex-displayed value as an integer "even for
  the float types".

### Changed

- `<VariableType>` now matches case-insensitively and trimmed, mirroring
  Cheat Engine's own `StringToVariableType` (`s := trim(lowercase(s))`).
  Reports still quote the original text, so an unrecognized type is named
  as the file actually wrote it.

### Notes on Cheat Engine compatibility

- CE reads `<ShowAsHex>` *before* it applies `<VariableType>`, and
  `setVarType` then forces hex on for a pointer — so the type wins over an
  explicit `<ShowAsHex>0</ShowAsHex>`, not the other way round.

Both types had been skipped since `0.1.0` with reasons saying their
structure wasn't verified against a real table. That blocker was cleared
while verifying the string details for `0.2.0`, which is why they land
now.

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

[Unreleased]: https://github.com/F3rNaNDEZ57/Ferrite/compare/v1.1.1...HEAD
[1.1.1]: https://github.com/F3rNaNDEZ57/Ferrite/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/F3rNaNDEZ57/Ferrite/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/F3rNaNDEZ57/Ferrite/compare/v0.3.0...v1.0.0
[0.3.0]: https://github.com/F3rNaNDEZ57/Ferrite/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/F3rNaNDEZ57/Ferrite/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v0.1.0
