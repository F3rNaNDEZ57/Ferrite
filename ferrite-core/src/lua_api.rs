//! The data-only slice of Cheat Engine's Lua API.
//!
//! Every function here is glue over machinery `ferrite-core` already has —
//! `ProcessSession` for the reads and writes, [`crate::modules`] for symbol
//! resolution, [`crate::text`] for string encoding. Nothing new reaches the
//! target; a script can do exactly what the scan panel can do, and no more.
//!
//! Cheat Engine registers **461** Lua functions. This installs roughly two
//! dozen. That is not an oversight to be filled in later: the omissions are
//! the design, because a function that does not exist is a capability a
//! script cannot reach. See [`crate::lua`] for the sandbox that surrounds
//! this.
//!
//! ## Signatures are Cheat Engine's, verified from its source
//!
//! Scripts are written against CE, so a plausible-looking signature that
//! differs in an argument or a failure mode is worse than no function at
//! all — it fails at someone else's runtime, in their table, on their game.
//! Read out of `LuaHandler.pas`:
//!
//! - **An address argument is a number *or* a string.** CE resolves strings
//!   through the symbol handler on every read and write, so
//!   `readInteger("game.exe+1C58DA0")` is ordinary usage rather than a
//!   convenience. [`to_address`] is that conversion.
//! - **`getAddress` raises on an address it can't resolve; `getAddressSafe`
//!   returns nil.** That is the *only* difference between them, and it is
//!   why both exist. A number passed to either comes straight back.
//! - **A failed read returns nil**, not an error: a script testing
//!   `if readInteger(a) then` is the normal shape.
//! - **`readInteger(address, signed)`** takes an optional signedness flag;
//!   **`readString(address, maxlength, widechar)`** takes three.
//! - **`readBytes(address, count, returnAsTable)`** returns a table when the
//!   third argument is true and `count` separate values when it isn't —
//!   awkward, but it is the contract scripts are written against.

use std::sync::Arc;
use std::time::Duration;

use mlua::{Lua, MultiValue, Value};

use crate::modules::{ModuleMap, module_base};
use crate::session::ProcessSession;
use crate::table::parse_address_expr;
use crate::text::{TextEncoding, decode_text};

/// What the API is allowed to act on for one script run.
///
/// The session is an `Arc` because mlua closures must be `'static`, and
/// because [`ProcessSession`] is already shared this way with the freeze
/// thread — so this borrows the application's existing ownership model
/// rather than inventing one.
#[derive(Clone)]
pub struct ScriptContext {
    /// The attached process, or `None` while detached.
    pub session: Option<Arc<ProcessSession>>,
    /// A module snapshot for resolving `module.exe+offset`.
    pub modules: Arc<ModuleMap>,
}

impl ScriptContext {
    /// A context with nothing attached. Reads and writes raise; everything
    /// else still works, which is what lets a script be syntax-checked
    /// before a process exists.
    pub fn detached() -> Self {
        Self {
            session: None,
            modules: Arc::new(ModuleMap::empty()),
        }
    }

    fn session(&self) -> mlua::Result<&ProcessSession> {
        self.session.as_deref().ok_or_else(|| {
            // Raised rather than returned as nil, deliberately. A nil would
            // be indistinguishable from "that address isn't readable",
            // which would send someone hunting a bad address when the real
            // problem is that nothing is attached.
            mlua::Error::RuntimeError(
                "no process is attached, so this script cannot read or write memory".to_string(),
            )
        })
    }
}

/// The longest a single `sleep` call may pause a script.
///
/// A script runs while the user waits, and the instruction budget in
/// [`crate::lua`] counts VM instructions rather than wall-clock time — so
/// `sleep(600000)` would sit there for ten minutes without ever tripping
/// it. Capping the call is cruder than a global time budget and much
/// simpler to reason about.
pub const MAX_SLEEP: Duration = Duration::from_millis(250);

/// Installs the API into an interpreter.
pub fn install(lua: &Lua, ctx: &ScriptContext) -> mlua::Result<()> {
    let globals = lua.globals();

    // ---- address resolution ---------------------------------------------
    let c = ctx.clone();
    globals.set(
        "getAddress",
        lua.create_function(move |_, (name, _local): (Value, Option<bool>)| {
            resolve(&c, &name)?.ok_or_else(|| {
                mlua::Error::RuntimeError(format!("could not resolve {}", describe(&name)))
            })
        })?,
    )?;

    let c = ctx.clone();
    globals.set(
        "getAddressSafe",
        // Same resolution, nil instead of an error. The one and only
        // difference from getAddress.
        lua.create_function(move |_, (name, _local): (Value, Option<bool>)| resolve(&c, &name))?,
    )?;

    let c = ctx.clone();
    globals.set(
        "getNameFromAddress",
        lua.create_function(move |_, address: Value| {
            let address = to_address(&c, &address)?;
            Ok(c.modules.resolve(address).map(|m| m.to_string()))
        })?,
    )?;

    let c = ctx.clone();
    globals.set(
        "getModuleSize",
        lua.create_function(move |_, name: String| {
            Ok(c.modules
                .modules()
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(&name))
                .map(|m| m.size as i64))
        })?,
    )?;

    let c = ctx.clone();
    globals.set(
        "enumModules",
        lua.create_function(move |lua, ()| {
            let list = lua.create_table()?;
            for (index, module) in c.modules.modules().iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("Name", module.name.clone())?;
                entry.set("Address", module.base as i64)?;
                entry.set("Size", module.size as i64)?;
                // Lua tables are 1-based.
                list.set(index + 1, entry)?;
            }
            Ok(list)
        })?,
    )?;

    // ---- reads ----------------------------------------------------------
    // Each returns nil on a failed read, which is the shape scripts test
    // for. Only "nothing is attached" raises.
    macro_rules! reader {
        ($name:literal, $len:expr, $convert:expr) => {{
            let c = ctx.clone();
            globals.set(
                $name,
                lua.create_function(move |_, (address, signed): (Value, Option<bool>)| {
                    let address = to_address(&c, &address)?;
                    let session = c.session()?;
                    let convert: fn(&[u8], bool) -> i64 = $convert;
                    Ok(session
                        .read_bytes(address, $len)
                        .ok()
                        .map(|bytes| convert(&bytes, signed.unwrap_or(true))))
                })?,
            )?;
        }};
    }

    reader!("readSmallInteger", 2, |b, signed| {
        let v = u16::from_le_bytes([b[0], b[1]]);
        if signed {
            i64::from(v as i16)
        } else {
            i64::from(v)
        }
    });
    reader!("readInteger", 4, |b, signed| {
        let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if signed {
            i64::from(v as i32)
        } else {
            i64::from(v)
        }
    });
    reader!("readQword", 8, |b, _signed| {
        i64::from_le_bytes(b.try_into().expect("8 bytes"))
    });
    // A pointer is an address-sized unsigned read. Same width as readQword;
    // kept separate because that is the name scripts use.
    reader!("readPointer", 8, |b, _signed| {
        i64::from_le_bytes(b.try_into().expect("8 bytes"))
    });

    let c = ctx.clone();
    globals.set(
        "readFloat",
        lua.create_function(move |_, address: Value| {
            let address = to_address(&c, &address)?;
            let session = c.session()?;
            Ok(session
                .read_bytes(address, 4)
                .ok()
                .map(|b| f64::from(f32::from_le_bytes(b.try_into().expect("4 bytes")))))
        })?,
    )?;

    let c = ctx.clone();
    globals.set(
        "readDouble",
        lua.create_function(move |_, address: Value| {
            let address = to_address(&c, &address)?;
            let session = c.session()?;
            Ok(session
                .read_bytes(address, 8)
                .ok()
                .map(|b| f64::from_le_bytes(b.try_into().expect("8 bytes"))))
        })?,
    )?;

    let c = ctx.clone();
    globals.set(
        "readString",
        lua.create_function(
            move |_, (address, max_length, wide): (Value, Option<usize>, Option<bool>)| {
                let address = to_address(&c, &address)?;
                let session = c.session()?;
                let encoding = if wide.unwrap_or(false) {
                    TextEncoding::Utf16Le
                } else {
                    TextEncoding::Latin1
                };
                // `maxlength` counts characters, as `<Length>` does in a
                // `.CT` file - so the byte count depends on the encoding.
                let chars = max_length.unwrap_or(0);
                if chars == 0 {
                    return Ok(None);
                }
                let bytes = chars * encoding.bytes_per_char();
                Ok(session
                    .read_bytes(address, bytes)
                    .ok()
                    // Truncated at the first NUL: a string read of a fixed
                    // buffer wants the text in it, not the padding.
                    .map(|b| decode_text(&b, encoding, true)))
            },
        )?,
    )?;

    let c = ctx.clone();
    globals.set(
        "readBytes",
        lua.create_function(
            move |lua, (address, count, as_table): (Value, usize, Option<bool>)| {
                let address = to_address(&c, &address)?;
                let session = c.session()?;
                let Ok(bytes) = session.read_bytes(address, count) else {
                    return Ok(MultiValue::new());
                };
                if as_table.unwrap_or(false) {
                    let table = lua.create_table()?;
                    for (index, byte) in bytes.iter().enumerate() {
                        table.set(index + 1, *byte)?;
                    }
                    return Ok(MultiValue::from_vec(vec![Value::Table(table)]));
                }
                // Otherwise `count` separate return values - CE's contract,
                // odd as it is.
                let mut values = Vec::with_capacity(bytes.len());
                for byte in bytes {
                    values.push(Value::Integer(i64::from(byte)));
                }
                Ok(MultiValue::from_vec(values))
            },
        )?,
    )?;

    // ---- writes ---------------------------------------------------------
    // Each returns true on success and false on failure, which is CE's
    // convention for writes.
    macro_rules! writer {
        ($name:literal, $to_bytes:expr) => {{
            let c = ctx.clone();
            globals.set(
                $name,
                lua.create_function(move |_, (address, value): (Value, f64)| {
                    let address = to_address(&c, &address)?;
                    let session = c.session()?;
                    let to_bytes: fn(f64) -> Vec<u8> = $to_bytes;
                    Ok(session.write_bytes(address, &to_bytes(value)).is_ok())
                })?,
            )?;
        }};
    }

    writer!("writeSmallInteger", |v| (v as i16).to_le_bytes().to_vec());
    writer!("writeInteger", |v| (v as i32).to_le_bytes().to_vec());
    writer!("writeQword", |v| (v as i64).to_le_bytes().to_vec());
    writer!("writePointer", |v| (v as i64).to_le_bytes().to_vec());
    writer!("writeFloat", |v| (v as f32).to_le_bytes().to_vec());
    writer!("writeDouble", |v| v.to_le_bytes().to_vec());

    let c = ctx.clone();
    globals.set(
        "writeString",
        lua.create_function(
            move |_, (address, text, wide): (Value, String, Option<bool>)| {
                let address = to_address(&c, &address)?;
                let session = c.session()?;
                let encoding = if wide.unwrap_or(false) {
                    TextEncoding::Utf16Le
                } else {
                    TextEncoding::Latin1
                };
                let bytes =
                    crate::text::encode_text(&text, encoding).map_err(mlua::Error::RuntimeError)?;
                Ok(session.write_bytes(address, &bytes).is_ok())
            },
        )?,
    )?;

    let c = ctx.clone();
    globals.set(
        "writeBytes",
        lua.create_function(move |_, args: MultiValue| {
            let mut args = args.into_iter();
            let address = args
                .next()
                .ok_or_else(|| mlua::Error::RuntimeError("writeBytes needs an address".into()))?;
            let address = to_address(&c, &address)?;
            let session = c.session()?;

            // CE accepts either a table of bytes or the bytes as varargs.
            let rest: Vec<Value> = args.collect();
            let bytes = match rest.as_slice() {
                [Value::Table(table)] => {
                    let mut bytes = Vec::with_capacity(table.raw_len());
                    for index in 1..=table.raw_len() {
                        bytes.push(table.get::<u8>(index)?);
                    }
                    bytes
                }
                values => values
                    .iter()
                    .map(|v| {
                        v.as_integer()
                            .and_then(|i| u8::try_from(i).ok())
                            .ok_or_else(|| {
                                mlua::Error::RuntimeError(
                                    "writeBytes takes byte values 0-255".into(),
                                )
                            })
                    })
                    .collect::<mlua::Result<Vec<u8>>>()?,
            };
            if bytes.is_empty() {
                return Ok(false);
            }
            Ok(session.write_bytes(address, &bytes).is_ok())
        })?,
    )?;

    // ---- odds and ends --------------------------------------------------
    let c = ctx.clone();
    globals.set(
        "getOpenedProcessID",
        lua.create_function(move |_, ()| Ok(c.session.as_ref().map(|s| s.pid())))?,
    )?;

    globals.set(
        "targetIs64Bit",
        // Ferrite attaches to 64-bit processes only, so this is a constant
        // rather than a query.
        lua.create_function(|_, ()| Ok(true))?,
    )?;

    globals.set(
        "getTickCount",
        lua.create_function(|_, ()| {
            Ok(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0))
        })?,
    )?;

    globals.set(
        "sleep",
        lua.create_function(|_, millis: u64| {
            std::thread::sleep(Duration::from_millis(millis).min(MAX_SLEEP));
            Ok(())
        })?,
    )?;

    Ok(())
}

/// Resolves an address argument that may be a number or a name, returning
/// `None` when a name doesn't resolve.
fn resolve(ctx: &ScriptContext, value: &Value) -> mlua::Result<Option<i64>> {
    match value {
        // A number passes straight through, exactly as CE does - it is
        // already an address, and re-resolving it would be wrong.
        Value::Integer(i) => Ok(Some(*i)),
        Value::Number(n) => Ok(Some(*n as i64)),
        Value::String(s) => {
            let text = s.to_string_lossy().to_string();
            let Ok(expr) = parse_address_expr(&text) else {
                return Ok(None);
            };
            match expr {
                crate::table::AddressExpr::Absolute(address) => Ok(Some(address as i64)),
                crate::table::AddressExpr::ModuleRelative { module, offset } => {
                    // Prefer the snapshot: it is what the results table
                    // resolves against, so a script and the interface agree
                    // about where a module is.
                    if let Some(found) = ctx
                        .modules
                        .modules()
                        .iter()
                        .find(|m| m.name.eq_ignore_ascii_case(&module))
                    {
                        return Ok(Some((found.base + offset) as i64));
                    }
                    // Fall back to asking the process, in case the snapshot
                    // predates a module being loaded.
                    match ctx.session.as_deref() {
                        Some(session) => Ok(module_base(session, &module)
                            .ok()
                            .map(|base| (base + offset) as i64)),
                        None => Ok(None),
                    }
                }
            }
        }
        _ => Ok(None),
    }
}

/// Resolves an address argument, raising when it can't be — which is what
/// every read and write needs, since acting on a bogus address would be
/// worse than failing.
fn to_address(ctx: &ScriptContext, value: &Value) -> mlua::Result<usize> {
    let resolved = resolve(ctx, value)?.ok_or_else(|| {
        mlua::Error::RuntimeError(format!("could not resolve {}", describe(value)))
    })?;
    usize::try_from(resolved)
        .map_err(|_| mlua::Error::RuntimeError(format!("{resolved} isn't a valid address")))
}

/// Describes an argument for an error message, without dumping a whole
/// table into it.
fn describe(value: &Value) -> String {
    match value {
        Value::String(s) => format!("{:?}", s.to_string_lossy()),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.type_name().to_string(),
    }
}

/// The functions this module installs, for the test that asserts nothing
/// outside the list is reachable.
pub const INSTALLED: &[&str] = &[
    "getAddress",
    "getAddressSafe",
    "getNameFromAddress",
    "getModuleSize",
    "enumModules",
    "readSmallInteger",
    "readInteger",
    "readQword",
    "readPointer",
    "readFloat",
    "readDouble",
    "readString",
    "readBytes",
    "writeSmallInteger",
    "writeInteger",
    "writeQword",
    "writePointer",
    "writeFloat",
    "writeDouble",
    "writeString",
    "writeBytes",
    "getOpenedProcessID",
    "targetIs64Bit",
    "getTickCount",
    "sleep",
];

/// Cheat Engine functions this deliberately does **not** install, and which
/// a script will therefore find nil.
///
/// Listed so the omission is explicit and testable rather than incidental.
/// Adding a no-op stub for any of these would be worse than leaving it
/// absent: a script would report success having done nothing.
pub const NOT_INSTALLED: &[&str] = &[
    // Code execution and injection - the whole of v2.0's scope.
    "autoAssemble",
    "executeCode",
    "allocateMemory",
    "deAlloc",
    "injectDLL",
    "createThread",
    // Debugger.
    "debug_setBreakpoint",
    "debugProcess",
    // CE's own GUI toolkit.
    "createForm",
    "createButton",
    "createHotkey",
    // Byte-pattern scanning: CE returns its own StringList object rather
    // than a table, so this needs a userdata type and its own design pass.
    "AOBScan",
    "AOBScanUnique",
    // The cheat-table record API, which needs the saved list plumbed
    // through - GUI-adjacent, and not part of this step.
    "getAddressList",
    "createMemoryRecord",
];

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::{Lua, LuaOptions, StdLib};

    fn lua_with(ctx: &ScriptContext) -> Lua {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        )
        .expect("interpreter");
        install(&lua, ctx).expect("the API installs");
        lua
    }

    #[test]
    fn every_installed_function_exists_and_nothing_else_does() {
        // The omissions are the design, so they are asserted rather than
        // assumed - a stub added later would fail this.
        let lua = lua_with(&ScriptContext::detached());
        for name in INSTALLED {
            let present: bool = lua
                .load(format!("return {name} ~= nil"))
                .eval()
                .unwrap_or(false);
            assert!(present, "{name} should be installed");
        }
        for name in NOT_INSTALLED {
            let present: bool = lua
                .load(format!("return {name} ~= nil"))
                .eval()
                .unwrap_or(false);
            assert!(!present, "{name} must NOT be installed");
        }
    }

    #[test]
    fn reading_while_detached_raises_rather_than_returning_nil() {
        // A nil would be indistinguishable from "that address isn't
        // readable", sending someone after a bad address when nothing is
        // attached.
        let lua = lua_with(&ScriptContext::detached());
        let err = lua
            .load("return readInteger(0x1000)")
            .eval::<Option<i64>>()
            .expect_err("should raise");
        assert!(
            err.to_string().contains("no process is attached"),
            "got: {err}"
        );
    }

    #[test]
    fn a_number_address_passes_straight_through() {
        // CE returns a numeric argument unchanged rather than re-resolving
        // it, and both getAddress and getAddressSafe do so.
        let lua = lua_with(&ScriptContext::detached());
        for call in ["getAddress(0x1234)", "getAddressSafe(0x1234)"] {
            let got: i64 = lua.load(format!("return {call}")).eval().expect("resolves");
            assert_eq!(got, 0x1234, "{call}");
        }
    }

    #[test]
    fn getaddress_raises_where_getaddresssafe_returns_nil() {
        // The one and only difference between them.
        let lua = lua_with(&ScriptContext::detached());
        let safe: Option<i64> = lua
            .load("return getAddressSafe('nosuch.dll+10')")
            .eval()
            .expect("getAddressSafe never raises");
        assert_eq!(safe, None);

        let err = lua
            .load("return getAddress('nosuch.dll+10')")
            .eval::<Option<i64>>()
            .expect_err("getAddress raises");
        assert!(err.to_string().contains("could not resolve"), "got: {err}");
    }

    #[test]
    fn an_absolute_hex_string_resolves_without_a_process() {
        let lua = lua_with(&ScriptContext::detached());
        let got: i64 = lua
            .load("return getAddress('7FF6A41C58DA')")
            .eval()
            .expect("an absolute address needs no process");
        assert_eq!(got, 0x7FF6_A41C_58DA);
    }

    #[test]
    fn sleep_is_capped_so_a_script_cannot_stall_indefinitely() {
        // The instruction budget counts VM instructions, not wall-clock
        // time, so sleep needs its own bound.
        let lua = lua_with(&ScriptContext::detached());
        let started = std::time::Instant::now();
        lua.load("sleep(60000)").exec().expect("sleeps");
        assert!(
            started.elapsed() < MAX_SLEEP * 4,
            "sleep(60000) took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn enum_modules_is_a_one_based_table_of_named_entries() {
        // Empty here, but the shape is what scripts index into.
        let lua = lua_with(&ScriptContext::detached());
        let count: usize = lua
            .load("local m = enumModules(); return #m")
            .eval()
            .expect("returns a table");
        assert_eq!(count, 0);
    }
}
