//! Runs cheat-table Lua against a real separate process.
//!
//! The unit tests in `lua.rs` and `lua_api.rs` prove the sandbox and the
//! signatures; this proves the thing that actually matters — that a script
//! written the way a Cheat Engine table writes one reads and writes another
//! process's memory correctly, through the interpreter, end to end.

mod common;

use std::sync::Arc;

use common::Victim;
use ferrite_core::{
    DEFAULT_INSTRUCTION_BUDGET, LuaError, ModuleMap, ProcessSession, ScriptContext, Section,
    parse_script, run_section,
};

/// Attaches to a fresh victim and builds the context a script would run in.
fn attached(victim: &Victim) -> ScriptContext {
    let session =
        Arc::new(ProcessSession::attach(victim.pid()).expect("attaching to the victim process"));
    let modules = Arc::new(ModuleMap::build(&session).expect("building the module map"));
    ScriptContext {
        session: Some(session),
        modules,
    }
}

fn run(script: &str, ctx: &ScriptContext, section: Section) -> ferrite_core::RunOutput {
    let parsed = parse_script(script).expect("the script parses");
    run_section(&parsed, section, ctx, false, DEFAULT_INSTRUCTION_BUDGET).expect("the script runs")
}

#[test]
fn a_script_reads_a_real_value_out_of_the_victim() {
    let victim = Victim::spawn();
    let ctx = attached(&victim);
    let hp = victim.address_of("HP");

    let out = run(
        &format!("{{$lua}}\nprint(readInteger({hp}))"),
        &ctx,
        Section::Enable,
    );
    assert_eq!(out.printed, vec!["100".to_string()]);
}

#[test]
fn a_script_writes_a_real_value_and_the_write_lands() {
    let victim = Victim::spawn();
    let ctx = attached(&victim);
    let hp = victim.address_of("HP");

    let out = run(
        &format!(
            "{{$lua}}\n\
             print(writeInteger({hp}, 1337))\n\
             print(readInteger({hp}))"
        ),
        &ctx,
        Section::Enable,
    );
    assert_eq!(out.printed, vec!["true".to_string(), "1337".to_string()]);

    // Read it back outside the interpreter, so this is proof the write
    // reached the process rather than that Lua agreed with itself.
    let session = ctx.session.as_deref().expect("attached");
    let bytes = session.read_bytes(hp, 4).expect("reading HP directly");
    assert_eq!(i32::from_le_bytes(bytes.try_into().unwrap()), 1337);
}

#[test]
fn an_enable_and_disable_pair_toggles_a_value_the_way_a_real_table_does() {
    // The shape of an actual cheat: a shared preamble resolving the address,
    // then one half that sets a value and one that restores it.
    let victim = Victim::spawn();
    let ctx = attached(&victim);
    let hp = victim.address_of("HP");
    let script = format!(
        "{{$lua}}\n\
         if syntaxcheck then return end\n\
         local health = {hp}\n\
         [ENABLE]\n\
         writeInteger(health, 9999)\n\
         [DISABLE]\n\
         writeInteger(health, 100)"
    );
    let parsed = parse_script(&script).expect("parses");

    let session = ctx.session.clone().expect("attached");
    let read_hp = || {
        let bytes = session.read_bytes(hp, 4).expect("reading HP");
        i32::from_le_bytes(bytes.try_into().unwrap())
    };

    assert_eq!(read_hp(), 100);
    run_section(
        &parsed,
        Section::Enable,
        &ctx,
        false,
        DEFAULT_INSTRUCTION_BUDGET,
    )
    .expect("enabling");
    assert_eq!(read_hp(), 9999, "enabling should have written");
    run_section(
        &parsed,
        Section::Disable,
        &ctx,
        false,
        DEFAULT_INSTRUCTION_BUDGET,
    )
    .expect("disabling");
    assert_eq!(read_hp(), 100, "disabling should have restored");
}

#[test]
fn a_syntax_check_runs_the_script_without_letting_it_act() {
    // The `if syntaxcheck then return end` idiom, doing its actual job:
    // Cheat Engine validates a script before running it, and a script that
    // acted during validation would apply a cheat nobody asked for.
    let victim = Victim::spawn();
    let ctx = attached(&victim);
    let hp = victim.address_of("HP");
    let parsed = parse_script(&format!(
        "{{$lua}}\n\
         if syntaxcheck then return end\n\
         writeInteger({hp}, 4242)"
    ))
    .expect("parses");

    run_section(
        &parsed,
        Section::Enable,
        &ctx,
        true,
        DEFAULT_INSTRUCTION_BUDGET,
    )
    .expect("the syntax check runs");

    let session = ctx.session.as_deref().expect("attached");
    let bytes = session.read_bytes(hp, 4).expect("reading HP");
    assert_eq!(
        i32::from_le_bytes(bytes.try_into().unwrap()),
        100,
        "a syntax check must not have written anything"
    );
}

#[test]
fn a_module_relative_address_resolves_through_the_snapshot() {
    // `module.exe+offset` is how nearly every real table addresses things,
    // so a script has to be able to use it rather than only raw addresses.
    let victim = Victim::spawn();
    let ctx = attached(&victim);
    let hp = victim.address_of("HP");

    let module = ctx
        .modules
        .resolve(hp)
        .expect("HP is a static inside the victim's own image");

    let out = run(
        &format!("{{$lua}}\nprint(readInteger('{module}'))"),
        &ctx,
        Section::Enable,
    );
    assert_eq!(out.printed, vec!["100".to_string()]);

    // ...and getAddress on the same expression lands on HP exactly.
    let out = run(
        &format!("{{$lua}}\nprint(getAddress('{module}') == {hp})"),
        &ctx,
        Section::Enable,
    );
    assert_eq!(out.printed, vec!["true".to_string()]);
}

#[test]
fn a_script_reads_the_victims_string_buffers_in_both_encodings() {
    let victim = Victim::spawn();
    let ctx = attached(&victim);
    let ascii = victim.address_of("STR_ASCII");
    let unicode = victim.address_of("STR_UNICODE");

    let out = run(
        &format!(
            "{{$lua}}\n\
             print(readString({ascii}, 13))\n\
             print(readString({unicode}, 13, true))"
        ),
        &ctx,
        Section::Enable,
    );
    assert_eq!(
        out.printed,
        vec!["FerriteVictim".to_string(), "FerriteVictim".to_string()],
        "the wide flag should pick the UTF-16 buffer"
    );
}

#[test]
fn read_and_write_bytes_round_trip_through_a_table() {
    let victim = Victim::spawn();
    let ctx = attached(&victim);
    let score = victim.address_of("SCORE");

    let out = run(
        &format!(
            "{{$lua}}\n\
             writeBytes({score}, {{1, 2, 3, 4}})\n\
             local b = readBytes({score}, 4, true)\n\
             print(b[1], b[2], b[3], b[4])"
        ),
        &ctx,
        Section::Enable,
    );
    assert_eq!(out.printed, vec!["1\t2\t3\t4".to_string()]);

    // ...and without the table flag it returns one value per byte, which is
    // Cheat Engine's other contract for the same function.
    let out = run(
        &format!("{{$lua}}\nprint(readBytes({score}, 4))"),
        &ctx,
        Section::Enable,
    );
    assert_eq!(out.printed, vec!["1\t2\t3\t4".to_string()]);
}

#[test]
fn a_failed_read_is_nil_rather_than_an_error() {
    // The shape a real script tests for: `if readInteger(a) then`. An error
    // here would make that idiom throw instead of branching.
    let victim = Victim::spawn();
    let ctx = attached(&victim);

    let out = run(
        "{$lua}\nprint(readInteger(0x10) == nil)",
        &ctx,
        Section::Enable,
    );
    assert_eq!(out.printed, vec!["true".to_string()]);
}

#[test]
fn the_sandbox_still_holds_with_a_live_process_attached() {
    // The sandbox is unit-tested detached; this confirms installing the
    // memory API didn't quietly reintroduce anything, which is exactly the
    // sort of regression a later refactor could cause.
    let victim = Victim::spawn();
    let ctx = attached(&victim);

    for name in [
        "io",
        "os",
        "package",
        "require",
        "debug",
        "dofile",
        "loadfile",
        "load",
        "autoAssemble",
        "executeCode",
        "allocateMemory",
        "injectDLL",
        "AOBScan",
    ] {
        let out = run(
            &format!("{{$lua}}\nprint({name} == nil)"),
            &ctx,
            Section::Enable,
        );
        assert_eq!(
            out.printed,
            vec!["true".to_string()],
            "{name} became reachable once the API was installed"
        );
    }
}

#[test]
fn an_assembler_script_is_refused_even_with_a_process_attached() {
    // The boundary that keeps a half-applied cheat from happening: the Lua
    // helper beside the assembly must not run.
    let victim = Victim::spawn();
    let ctx = attached(&victim);
    let hp = victim.address_of("HP");

    let parsed = parse_script(&format!(
        "[ENABLE]\n\
         {{$lua}}\n\
         writeInteger({hp}, 777)\n\
         {{$asm}}\n\
         mov [rsi+1C],#100"
    ))
    .expect("parses");

    let result = run_section(
        &parsed,
        Section::Enable,
        &ctx,
        false,
        DEFAULT_INSTRUCTION_BUDGET,
    );
    assert!(
        matches!(result, Err(LuaError::NotRunnable(_))),
        "expected a refusal, got {result:?}"
    );

    let session = ctx.session.as_deref().expect("attached");
    let bytes = session.read_bytes(hp, 4).expect("reading HP");
    assert_eq!(
        i32::from_le_bytes(bytes.try_into().unwrap()),
        100,
        "the refused script's Lua must not have written"
    );
}
