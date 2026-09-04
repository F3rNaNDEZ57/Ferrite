#!/usr/bin/env python3
"""Mirrors the planning vault to the repository's GitHub wiki.

    python tools/mirror-wiki.py          # build the pages
    python tools/mirror-wiki.py --push   # ...and commit and push them

The vault is the editing source and the wiki is a copy: a page edited on
the wiki directly is overwritten by the next mirror. Notes are ported
verbatim under their own file names so the vault's `[[wikilinks]]` keep
resolving - GitHub wikis use the same syntax and take a page's name from
the file stem. `index.md` is the one exception: `Home.md` replaces it, and
`[[index]]` is rewritten to `[[Home]]`.

A wiki page whose vault note has gone is deleted rather than left live
beside its replacement, which is how a rename cleans up after itself.
"""

import io, os, glob, subprocess, sys

# Paths resolve relative to this file, so the script works from any
# checkout: tools/mirror-wiki.py -> ../../vault, with the wiki cloned
# into target/wiki. Both are overridable for an unusual layout.
HERE = os.path.dirname(os.path.abspath(__file__))
VAULT = os.environ.get("FERRITE_VAULT", os.path.join(HERE, "..", "..", "vault"))
WIKI = os.environ.get("FERRITE_WIKI", os.path.join(HERE, "..", "target", "wiki"))
WIKI_REMOTE = "https://github.com/F3rNaNDEZ57/Ferrite.wiki.git"

# Every vault note except index.md, which Home.md replaces. Ported verbatim
# under their existing file names so the vault's own [[wikilinks]] keep
# resolving — GitHub wikis use the same syntax and derive the page name from
# the file stem.
NOTES = [
    "progress-log.md",
    "v0.1-scope.md",
    "v0.1-plan.md",
    "v0.2-scope.md",
    "v0.2-plan.md",
    "v0.3-notes.md",
    "v1.0-notes.md",
    "v1.1-scope.md",
    "v1.1-plan.md",
    "v2.0-scope.md",
]


def banner(stem):
    # Bold rather than italics: an emphasis pair spanning several sentences
    # is easy to leave unbalanced, and the previous version rendered a stray
    # underscore into the live page.
    return (
        "> **Mirrored from the project's planning vault.** "
        f"(`vault/{stem}`) These are working notes kept alongside the code, "
        "so they read as notes: they record what was decided and why, "
        "including the mistakes. Edits belong in the vault — a change made "
        "here is overwritten on the next mirror.\n\n"
    )


HOME = """# Ferrite

An open-source, Rust-native memory scanner for Windows — a clean-room
reimplementation of the Cheat Engine idea. Attach to a running process,
scan its memory for a value, narrow the results by rescanning as that value
changes, then edit or freeze what you found and save it as a reusable
table.

It reads and writes process **data**. It never injects or executes code
**in the target process**, never touches the network, and collects
nothing.

**Current release: [v1.0.0](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v1.0.0)**
· [Download](https://github.com/F3rNaNDEZ57/Ferrite/releases/latest)
· [Changelog](https://github.com/F3rNaNDEZ57/Ferrite/blob/main/CHANGELOG.md)

## What this wiki is

The project's planning vault, mirrored. It isn't polished documentation —
it's the working record: what each release scoped, what was decided and
why, what was verified and how, and the things that turned out to be wrong.
If you want to know *why* Ferrite does something a particular way, the
answer is usually here rather than in a code comment.

For how to build and run it, see the
[README](https://github.com/F3rNaNDEZ57/Ferrite#readme).

## Releases

| Version | What it was | Notes |
| --- | --- | --- |
| [v1.0.0](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v1.0.0) | The interface rebuilt: three docked regions, a virtualised results table, fixed-width aligned hex, and the `.CT` import report as a split view for reading a downloaded table's script before trusting it | [[v1.0-notes]] |
| [v0.3.0](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v0.3.0) | `Pointer` and `Array of byte` `.CT` entries, `<ShowAsHex>`, case-insensitive type names | [[v0.3-notes]] |
| [v0.2.0](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v0.2.0) | String value types, multi-level pointer chains, script-text display for entries Ferrite can't run | [[v0.2-scope]] · [[v0.2-plan]] |
| [v0.1.0](https://github.com/F3rNaNDEZ57/Ferrite/releases/tag/v0.1.0) | The core loop: attach → scan → filter → edit/freeze → save/load, plus `.CT` import | [[v0.1-scope]] · [[v0.1-plan]] |

## Planned

| Version | What it would be | Notes |
| --- | --- | --- |
| `v1.1.0` | Run the data-only subset of `{$LUA}` cheat-table scripts, sandboxed | [[v1.1-scope]] · [[v1.1-plan]] |
| `v2.0.0` | Auto Assembler execution — assembling, allocation in the target, code injection | [[v2.0-scope]] |

These two are often asked for as one thing — "run the Lua cheats" — and
they are not. A `{$LUA}` script runs **in Ferrite's own process** and acts
on the target through ordinary reads and writes, which is what Ferrite
already does. An Auto Assembler script allocates memory **inside the
target**, writes machine code there, and patches the target's execution to
run it.

So v1.1.0 keeps the promise that Ferrite never injects or executes code in
the target; v2.0.0 would retire it, which is why it is a major version and
a decision rather than a schedule. Most god-mode cheats in downloaded
tables are the second kind — [[v1.1-scope]] says so up front rather than
letting you discover it.

## How these notes are named

A release planned in advance has a **`-scope`** and a **`-plan`** note; a
release taken without a planning pass has a single **`-notes`** note
written afterwards. The difference is real rather than cosmetic — v0.3.0
was a contained follow-on whose blocker had already been cleared, and
v1.0.0 was built from a design specification produced outside the vault.

> **Careful with "v1".** [[v0.1-scope]] and [[v0.1-plan]] say "v1"
> throughout, meaning the project's original first-release milestone —
> which shipped as `v0.1.0`. They are **not** about `v1.0.0`, which came
> much later and is [[v1.0-notes]].

## Start here

| If you want to… | Read |
| --- | --- |
| See what's built, verified, and open | [[progress-log]] |
| Understand what Ferrite is and isn't | [[v0.1-scope]] |
| Understand how it was built, and the decisions log | [[v0.1-plan]] |
| Read the Cheat Engine format findings | [[v0.2-plan]] · [[v0.3-notes]] |
| See how the current interface was designed and verified | [[v1.0-notes]] |
| Know whether Ferrite will run a table's Lua scripts | [[v1.1-scope]] |
| Understand what running Auto Assembler cheats would cost | [[v2.0-scope]] |

## Two things worth knowing

**On Cheat Engine compatibility.** Ferrite imports real `.CT` files, and
every detail of that format documented here was read out of Cheat Engine's
own source (`MemoryRecordUnit.pas`, `CEFuncProc.pas`) rather than inferred.
That's deliberate: the ones that matter — `<Length>` counting *characters*
rather than bytes, an absent `<ZeroTerminate>` defaulting to *true*,
pointer offsets being stored in document order but walked last-to-first —
all fail **silently** if guessed wrong, producing a plausible-looking wrong
answer instead of an error. See [[v0.2-plan]] and [[v0.3-notes]].

**On scripts.** A `.CT` file can carry Auto Assembler or Lua scripts. As of
v1.0.0 Ferrite runs none of them: the import report shows you a skipped
entry's script *text*, in full, so you can read what it would have done and
decide for yourself.

That caution is not squeamishness. Downloaded cheat tables with embedded
scripts are a documented malware vector, and the one real sample inspected
while planning v0.2.0 turned out to be ad injection rather than a cheat.
[[v1.1-scope]] proposes running the *data-only* kind, and the reason that
is defensible is worth reading before assuming it's a reversal: the
guarantee is that the functions which could inject code **do not exist in
the interpreter**, rather than that Ferrite inspected the script and judged
it safe.

## Not planned

Auto Assembler execution is **scoped but not agreed** — see [[v2.0-scope]]
for what it would take and the decision it demands. The debugger, kernel
drivers and structure dissect remain out of scope entirely. Everything else
still open is listed at the top of [[progress-log]].
"""

SIDEBAR = """### [Ferrite](https://github.com/F3rNaNDEZ57/Ferrite)

**[[Home]]**

**Status**
- [[progress-log]]

**Planned**
- [[v1.1-scope]]
- [[v1.1-plan]]
- [[v2.0-scope]]

**v1.0.0**
- [[v1.0-notes]]

**v0.3.0**
- [[v0.3-notes]]

**v0.2.0**
- [[v0.2-scope]]
- [[v0.2-plan]]

**v0.1.0**
- [[v0.1-scope]]
- [[v0.1-plan]]

---

- [Releases](https://github.com/F3rNaNDEZ57/Ferrite/releases)
- [Changelog](https://github.com/F3rNaNDEZ57/Ferrite/blob/main/CHANGELOG.md)
- [README](https://github.com/F3rNaNDEZ57/Ferrite#readme)
"""

FOOTER = (
    "_Mirrored from the planning vault in the "
    "[Ferrite](https://github.com/F3rNaNDEZ57/Ferrite) repository. "
    "Licensed MIT OR Apache-2.0, like the code._\n"
)

# Clear out any page from a previous mirror that no longer has a vault note
# behind it, so a rename doesn't leave the old page live alongside the new
# one. Home/_Sidebar/_Footer are generated, not mirrored.
if not os.path.isdir(os.path.join(WIKI, ".git")):
    os.makedirs(os.path.dirname(os.path.abspath(WIKI)), exist_ok=True)
    subprocess.run(["git", "clone", WIKI_REMOTE, WIKI], check=True)

keep = set(NOTES) | {"Home.md", "_Sidebar.md", "_Footer.md"}
removed = []
for existing in glob.glob(os.path.join(WIKI, "*.md")):
    if os.path.basename(existing) not in keep:
        os.remove(existing)
        removed.append(os.path.basename(existing))

written = []
for stem in NOTES:
    body = io.open(os.path.join(VAULT, stem), encoding="utf-8").read()
    # The vault's entry point is index.md; the wiki's is Home. That is the
    # one link the mirror has to translate rather than pass through.
    body = body.replace("[[index]]", "[[Home]]")
    dst = os.path.join(WIKI, stem)
    io.open(dst, "w", encoding="utf-8", newline="\n").write(banner(stem) + body)
    written.append((stem, os.path.getsize(dst)))

for name, content in (("Home.md", HOME), ("_Sidebar.md", SIDEBAR), ("_Footer.md", FOOTER)):
    path = os.path.join(WIKI, name)
    io.open(path, "w", encoding="utf-8", newline="\n").write(content)
    written.append((name, os.path.getsize(path)))

if removed:
    print("removed stale pages:", removed)
for name, size in written:
    print(f"{size:>8}  {name}")


# ---------------------------------------------------------------------------
if "--push" in sys.argv:
    def git(*args):
        subprocess.run(["git", "-C", WIKI, *args], check=True)

    git("add", "-A")
    dirty = subprocess.run(
        ["git", "-C", WIKI, "status", "--porcelain"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    if dirty:
        git("commit", "-m", "Mirror the planning vault")
        git("push", "origin", "master")
        print("pushed")
    else:
        print("wiki already matches the vault; nothing to push")
