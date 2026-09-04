//! A deliberately crippled Lua interpreter for running the data-only
//! `{$LUA}` blocks of a cheat-table script.
//!
//! This module provides the *environment*: the sandbox, the instruction
//! budget, and the `{$LUA}` block plumbing. The Cheat Engine API a script
//! calls — `readInteger`, `writeInteger` and the rest — lives in
//! [`crate::lua_api`], which is installed into each interpreter here.
//!
//! ## The safety model, and exactly what backs it
//!
//! The claim is **not** "Ferrite inspects a script and judges it safe". It
//! is that the operations which could reach the machine, the filesystem or
//! the target's execution **are not present in the environment**. A script
//! reaching for one fails on a nil value.
//!
//! That claim is delivered by two mechanisms, and the distinction is worth
//! keeping straight because one of them is subtractive:
//!
//! | Absent by omission — never loaded | Absent by removal — in `base`, always loaded |
//! | --- | --- |
//! | `io`, `os`, `package`, `require`, `debug` | `load`, `loadstring`, `dofile`, `loadfile` |
//!
//! Choosing the standard libraries explicitly leaves the first column out.
//! The second column ships inside Lua's base library, which is not
//! optional, so those globals are set to nil after construction.
//! [`REMOVED_GLOBALS`] is the list, and a test walks it.
//!
//! `dofile` and `loadfile` are the ones that matter most: both read and
//! execute a Lua file from disk, which would be a filesystem escape sitting
//! inside a sandbox advertised as data-only. An earlier draft of the plan
//! assumed all of these were simply never loaded. They are not.
//!
//! **What this does not remove.** Once the memory API exists, a script will
//! be able to write to any address in any process Ferrite has open. That is
//! what a cheat *is*, so it can't be designed away — which is why
//! "sandboxed" must never be presented to a user as "harmless", and why
//! running a script is a consented action rather than a side effect of
//! importing one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, VmState};

use crate::lua_api::{self, ScriptContext};
use crate::script::{Script, ScriptKind, Section};

/// Globals that exist in Lua's base library and have to be removed rather
/// than left out. Public so the safety test can assert over the same list
/// the constructor uses, instead of a copy that could drift from it.
pub const REMOVED_GLOBALS: &[&str] = &[
    // Each of these loads and runs code, and the first two read the disk.
    "dofile",
    "loadfile",
    "load",
    "loadstring",
    // Not a security hole, but a script has no business driving Ferrite's
    // collector, and it is trivially a way to stall.
    "collectgarbage",
];

/// How many Lua VM instructions a single script run may execute.
///
/// A script is someone else's code running while the user waits, so
/// `while true do end` has to end in a bounded time rather than wedging the
/// application. Ten million is far beyond anything a value-poking cheat
/// needs and still terminates in well under a second.
pub const DEFAULT_INSTRUCTION_BUDGET: u64 = 10_000_000;

/// How often the budget is checked, in VM instructions. Checking every
/// instruction would dominate the runtime; every few thousand is
/// indistinguishable in wall-clock terms for a runaway loop.
const HOOK_INTERVAL: u32 = 4_096;

/// Why running a script's Lua failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaError {
    /// The script isn't the kind this interpreter will run. Carries what it
    /// was classified as, so a caller can explain rather than just refuse.
    ///
    /// Checked here as well as in the GUI on purpose: this is the boundary
    /// that keeps an assembler script from having its Lua half executed,
    /// and a boundary enforced in one place only is one refactor away from
    /// not being enforced.
    NotRunnable(ScriptKind),
    /// The instruction budget ran out — most likely an endless loop.
    BudgetExhausted { budget: u64 },
    /// The script doesn't compile, so **nothing ran**.
    ///
    /// Separate from [`Self::Runtime`] because the difference is the whole
    /// question a caller needs answered after a failed enable: a script that
    /// never started leaves the target untouched, while one that failed
    /// part-way through may have written some of what it intended. Only the
    /// second needs reporting as suspect.
    Syntax(String),
    /// The script raised while running, so an unknown amount of it took
    /// effect. See [`Self::Syntax`] for why this is a separate variant.
    Runtime(String),
}

impl LuaError {
    /// Whether the script may have partly taken effect before failing.
    ///
    /// `false` only when nothing can have run. Anything else is `true`,
    /// including the budget case — a script stopped mid-loop has already
    /// done whatever it did before the loop.
    pub fn may_have_acted(&self) -> bool {
        match self {
            Self::NotRunnable(_) | Self::Syntax(_) => false,
            Self::BudgetExhausted { .. } | Self::Runtime(_) => true,
        }
    }
}

impl std::fmt::Display for LuaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunnable(kind) => {
                write!(f, "this script won't be run: {}", kind.reason())
            }
            Self::BudgetExhausted { budget } => write!(
                f,
                "the script was stopped after {budget} operations — it may not terminate"
            ),
            Self::Syntax(message) | Self::Runtime(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for LuaError {}

/// What a script run produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    /// Whatever the script passed to `print`, one entry per call. A script's
    /// own output is often how it tells the user what it did, and Ferrite
    /// has no console to send it to.
    pub printed: Vec<String>,
    /// The string a block returned, if it returned one.
    ///
    /// Cheat Engine would substitute this into the script as assembly. This
    /// interpreter never assembles anything, so a returned string means the
    /// script is generating code and the run is abandoned — see
    /// [`run_section`]. Kept rather than discarded so the caller can say
    /// what it found.
    pub returned: Option<String>,
}

/// Runs one half of a script's `{$LUA}` blocks.
///
/// `syntax_check` is passed to each block as Cheat Engine passes it: `true`
/// asks a script to validate itself without acting. Real scripts branch on
/// it, overwhelmingly as `if syntaxcheck then return end`.
///
/// Blocks run in source order, each in its **own** interpreter. Sharing one
/// would let a block leave state behind for the next, and a script's second
/// block reading the first's globals is behaviour this cannot promise to
/// reproduce faithfully — better to be predictably strict than accidentally
/// compatible.
///
/// Stops at the first block that returns a string, reporting
/// [`RunOutput::returned`], because such a block is generating assembly and
/// continuing would apply half a cheat. The classifier should have caught
/// this before now; this is the backstop that does not depend on a source
/// scan being right.
pub fn run_section(
    script: &Script,
    section: Section,
    ctx: &ScriptContext,
    syntax_check: bool,
    budget: u64,
) -> Result<RunOutput, LuaError> {
    let kind = script.kind();
    if !kind.is_runnable() {
        return Err(LuaError::NotRunnable(kind));
    }

    let mut output = RunOutput::default();
    for block in script.lua_blocks(section) {
        let printed = Arc::new(Mutex::new(Vec::new()));
        let lua = sandboxed(Arc::clone(&printed), budget)?;
        // The API goes in after the sandbox, so a name it installs can
        // never be one the sandbox was meant to have removed.
        lua_api::install(&lua, ctx).map_err(|e| LuaError::Runtime(e.to_string()))?;

        // Prepend exactly what Cheat Engine prepends. Real scripts refer to
        // these by name — `if syntaxcheck then return end` is the standard
        // opening line — so binding them positionally instead would break
        // every script written against CE.
        let chunk = format!("local syntaxcheck,memrec=...\n{block}");
        let result = lua
            .load(&chunk)
            .set_name("cheat table script")
            .call::<Option<String>>((syntax_check, mlua::Nil));

        output
            .printed
            .append(&mut printed.lock().expect("print buffer isn't poisoned"));

        match result {
            Ok(None) => {}
            Ok(Some(returned)) => {
                output.returned = Some(returned);
                return Ok(output);
            }
            Err(err) => return Err(classify_lua_error(&err, budget)),
        }
    }
    Ok(output)
}

/// Builds the interpreter described in this module's documentation.
fn sandboxed(printed: Arc<Mutex<Vec<String>>>, budget: u64) -> Result<Lua, LuaError> {
    // Only these three libraries are loaded. `base` comes along regardless
    // — it is where the globals in `REMOVED_GLOBALS` live.
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH,
        LuaOptions::default(),
    )
    .map_err(|e| LuaError::Runtime(e.to_string()))?;

    {
        let globals = lua.globals();
        for name in REMOVED_GLOBALS {
            globals
                .set(*name, mlua::Nil)
                .map_err(|e| LuaError::Runtime(e.to_string()))?;
        }

        // `print` is replaced rather than removed: a script's output is
        // information the user wants, and sending it to a stdout no GUI has
        // would throw it away.
        let print = lua
            .create_function(move |_, args: mlua::MultiValue| {
                // `Value::to_string` follows Lua's own `tostring`, including
                // any `__tostring` metamethod. Rust's `{:?}` would print
                // `Integer(3)` and `Boolean(true)` where a user reading a
                // script's output expects `3` and `true`.
                let line = args
                    .iter()
                    .map(|v| {
                        v.to_string()
                            .unwrap_or_else(|_| "<unprintable>".to_string())
                    })
                    .collect::<Vec<_>>()
                    .join("\t");
                printed
                    .lock()
                    .expect("print buffer isn't poisoned")
                    .push(line);
                Ok(())
            })
            .map_err(|e| LuaError::Runtime(e.to_string()))?;
        globals
            .set("print", print)
            .map_err(|e| LuaError::Runtime(e.to_string()))?;
    }

    // The budget. The hook fires every HOOK_INTERVAL instructions and
    // raises once the allowance is gone; raising is what unwinds the script
    // rather than letting it continue.
    //
    // The counter is atomic because mlua takes the hook as an `Fn` — it may
    // be called from wherever the VM happens to be — so a plain captured
    // integer can't be incremented.
    let used = Arc::new(AtomicU64::new(0));
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_lua, _debug| {
            let spent = used.fetch_add(u64::from(HOOK_INTERVAL), Ordering::Relaxed)
                + u64::from(HOOK_INTERVAL);
            if spent > budget {
                return Err(mlua::Error::RuntimeError(BUDGET_MARKER.to_string()));
            }
            Ok(VmState::Continue)
        },
    )
    .map_err(|e| LuaError::Runtime(e.to_string()))?;

    Ok(lua)
}

/// Marks the budget error so it can be told apart from a script's own
/// `error()` call once Lua has wrapped it in position information.
const BUDGET_MARKER: &str = "ferrite:instruction-budget-exhausted";

fn classify_lua_error(err: &mlua::Error, budget: u64) -> LuaError {
    let text = err.to_string();
    if text.contains(BUDGET_MARKER) {
        return LuaError::BudgetExhausted { budget };
    }
    // A syntax error means the chunk never began executing, which is what
    // lets a caller distinguish "nothing happened" from "something did".
    if matches!(err, mlua::Error::SyntaxError { .. }) {
        return LuaError::Syntax(text);
    }
    LuaError::Runtime(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::parse_script;

    fn run(source: &str) -> Result<RunOutput, LuaError> {
        let script = parse_script(source).expect("the fixture parses");
        run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            false,
            DEFAULT_INSTRUCTION_BUDGET,
        )
    }

    /// Evaluates an expression inside the sandbox and reports whether it was
    /// truthy, via `print` — the only channel out of the interpreter.
    fn probe(expression: &str) -> bool {
        let out = run(&format!("{{$lua}}\nprint({expression})")).expect("the probe runs");
        out.printed == vec!["true".to_string()]
    }

    #[test]
    fn the_dangerous_globals_are_not_reachable() {
        // The whole safety claim, as a test. Both mechanisms are covered:
        // the omitted libraries and the removed base-library globals.
        for name in ["io", "os", "package", "require", "debug"] {
            assert!(!probe(&format!("{name} ~= nil")), "{name} was reachable");
        }
        for name in REMOVED_GLOBALS {
            assert!(!probe(&format!("{name} ~= nil")), "{name} was reachable");
        }
    }

    #[test]
    fn the_obvious_routes_back_are_closed() {
        // Removal is subtractive, so it is worth proving a script cannot
        // simply fetch the globals another way.
        for route in [
            "_G.io ~= nil",
            "_G['os'] ~= nil",
            "rawget(_G, 'dofile') ~= nil",
            "_ENV.load ~= nil",
            "getmetatable(_G) ~= nil",
        ] {
            assert!(!probe(route), "{route} found a way back");
        }
    }

    #[test]
    fn ordinary_lua_still_works() {
        // A sandbox that broke the language would be no use: a data-only
        // script still needs arithmetic, tables, strings and control flow.
        let out = run("{$lua}\n\
             local t = {3, 1, 2}\n\
             table.sort(t)\n\
             print(string.format('%d-%d-%d', t[1], t[2], t[3]))\n\
             print(math.floor(7 / 2))\n\
             print(pcall(function() error('caught') end))")
        .expect("runs");
        assert_eq!(out.printed[0], "1-2-3");
        assert_eq!(out.printed[1], "3");
        // `pcall` returns two values and Lua's `print` shows both,
        // tab-separated - so this also pins multi-value printing.
        assert!(
            out.printed[2].starts_with("false	") && out.printed[2].ends_with("caught"),
            "got: {}",
            out.printed[2]
        );
    }

    #[test]
    fn print_is_captured_rather_than_lost() {
        let out = run("{$lua}\nprint('one')\nprint('two', 'three')").expect("runs");
        assert_eq!(
            out.printed,
            vec!["one".to_string(), "two\tthree".to_string()]
        );
    }

    #[test]
    fn a_runaway_loop_is_stopped_by_the_budget() {
        // The reason the budget exists: this is someone else's code running
        // while the user waits.
        let script = parse_script("{$lua}\nwhile true do end").expect("parses");
        let result = run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            false,
            100_000,
        );
        assert_eq!(
            result,
            Err(LuaError::BudgetExhausted { budget: 100_000 }),
            "an endless loop must terminate"
        );
    }

    #[test]
    fn a_scripts_own_error_is_not_reported_as_a_budget_failure() {
        // Both arrive as an mlua::Error; conflating them would tell the user
        // their script loops when it actually threw.
        let err = run("{$lua}\nerror('deliberate')").expect_err("errors");
        match err {
            LuaError::Runtime(message) => {
                assert!(message.contains("deliberate"), "got: {message}")
            }
            other => panic!("expected a runtime error, got {other:?}"),
        }
    }

    #[test]
    fn the_syntaxcheck_and_memrec_arguments_are_bound_by_name() {
        // Real scripts refer to these by name, so binding them positionally
        // would break every script written against Cheat Engine.
        let script =
            parse_script("{$lua}\nprint(syntaxcheck)\nprint(memrec == nil)").expect("parses");
        let out = run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            true,
            DEFAULT_INSTRUCTION_BUDGET,
        )
        .expect("runs");
        assert_eq!(out.printed, vec!["true".to_string(), "true".to_string()]);
    }

    #[test]
    fn the_standard_preamble_returns_early_without_being_an_error() {
        // `if syntaxcheck then return end` opens nearly every real script.
        // It must run cleanly and do nothing, not look like a failure.
        let script = parse_script(
            "{$lua}\n\
             if syntaxcheck then return end\n\
             print('acted')",
        )
        .expect("parses");

        let checking = run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            true,
            DEFAULT_INSTRUCTION_BUDGET,
        )
        .expect("runs under a syntax check");
        assert!(checking.printed.is_empty(), "it should not have acted");
        assert_eq!(checking.returned, None);

        let acting = run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            false,
            DEFAULT_INSTRUCTION_BUDGET,
        )
        .expect("runs for real");
        assert_eq!(acting.printed, vec!["acted".to_string()]);
    }

    #[test]
    fn each_half_runs_only_its_own_blocks() {
        // The ordering CE uses, exercised end to end: the shared preamble
        // reaches both halves and neither half sees the other's body.
        let script = parse_script(
            "{$lua}\n\
             local shared = 'preamble'\n\
             [ENABLE]\n\
             print('on: ' .. shared)\n\
             [DISABLE]\n\
             print('off: ' .. shared)",
        )
        .expect("parses");

        let on = run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            false,
            DEFAULT_INSTRUCTION_BUDGET,
        )
        .expect("runs");
        assert_eq!(on.printed, vec!["on: preamble".to_string()]);

        let off = run_section(
            &script,
            Section::Disable,
            &ScriptContext::detached(),
            false,
            DEFAULT_INSTRUCTION_BUDGET,
        )
        .expect("runs");
        assert_eq!(off.printed, vec!["off: preamble".to_string()]);
    }

    #[test]
    fn an_unrunnable_script_is_refused_before_anything_executes() {
        // The boundary, enforced here and not only in the GUI. If an
        // assembler script's Lua helper ran, the target would be left
        // half-modified - the failure the classifier exists to prevent.
        let script = parse_script(
            "[ENABLE]\n\
             {$lua}\n\
             print('this must not run')\n\
             {$asm}\n\
             mov eax,1",
        )
        .expect("parses");
        let result = run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            false,
            DEFAULT_INSTRUCTION_BUDGET,
        );
        assert_eq!(result, Err(LuaError::NotRunnable(ScriptKind::Assembler)));
    }

    #[test]
    fn a_syntax_error_is_told_apart_from_a_runtime_one() {
        // The distinction the GUI needs after a failed enable: a script that
        // never compiled left the target untouched, while one that raised
        // part-way through may have written some of what it intended.
        let broken = run("{$lua}\nthis is not lua at all").expect_err("fails");
        assert!(
            matches!(broken, LuaError::Syntax(_)),
            "expected a syntax error, got {broken:?}"
        );
        assert!(!broken.may_have_acted(), "nothing can have run");

        let raised = run("{$lua}\nprint('acted')\nerror('then failed')").expect_err("fails");
        assert!(
            matches!(raised, LuaError::Runtime(_)),
            "expected a runtime error, got {raised:?}"
        );
        assert!(raised.may_have_acted(), "the print already happened");

        // The budget counts as having acted too - a script stopped mid-loop
        // has already done whatever preceded the loop.
        let script = parse_script("{$lua}\nwhile true do end").expect("parses");
        let stopped = run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            false,
            100_000,
        )
        .expect_err("fails");
        assert!(stopped.may_have_acted());

        // ...and a refusal did not, by definition.
        assert!(!LuaError::NotRunnable(ScriptKind::Assembler).may_have_acted());
    }

    #[test]
    fn a_generative_script_never_reaches_the_interpreter() {
        // A block that returns a string is Cheat Engine's code-generation
        // mechanism, and the guard in `run_section` refuses it before any of
        // its Lua runs - which matters because its side effects would
        // otherwise land while the assembly it produced went nowhere.
        let script = Script {
            enable: vec![crate::script::Block::Lua(
                "print('side effect')\nreturn 'mov eax,1'".to_string(),
            )],
            disable: Vec::new(),
        };
        assert_eq!(script.kind(), ScriptKind::GenerativeLua);
        assert_eq!(
            run_section(
                &script,
                Section::Enable,
                &ScriptContext::detached(),
                false,
                DEFAULT_INSTRUCTION_BUDGET,
            ),
            Err(LuaError::NotRunnable(ScriptKind::GenerativeLua)),
            "the side effect must not have run"
        );
    }

    // `RunOutput::returned` is deliberately not covered by a test, and the
    // reason is worth writing down rather than leaving as a gap: it is
    // unreachable today. The guard above rejects every script the classifier
    // calls generative, and the classifier uses the same source scan that
    // would have to be wrong for a returned string to get this far. It is
    // kept as a second line of defence for a hole in that scan nobody has
    // thought of - so a test could only exercise it by faking the very
    // mismatch it exists to catch.

    #[test]
    fn one_block_cannot_leave_state_for_the_next() {
        // Each block gets its own interpreter, so a second block reading the
        // first's globals sees nothing. Predictably strict beats
        // accidentally compatible.
        let script = parse_script(
            "{$lua}\n\
             leaked = 'first'\n\
             {$asm}\n\
             // comments only, so this stays a data-only script\n\
             {$lua}\n\
             print(leaked == nil)",
        )
        .expect("parses");
        assert_eq!(script.kind(), ScriptKind::DataOnlyLua);

        let out = run_section(
            &script,
            Section::Enable,
            &ScriptContext::detached(),
            false,
            DEFAULT_INSTRUCTION_BUDGET,
        )
        .expect("runs");
        assert_eq!(out.printed, vec!["true".to_string()]);
    }
}
