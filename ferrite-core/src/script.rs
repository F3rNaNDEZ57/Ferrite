//! Parsing and classifying a `.CT` entry's `<AssemblerScript>`.
//!
//! This module does **not** execute anything. It answers one question: what
//! kind of script is this, and could Ferrite ever run it? That question has
//! to be answerable before an interpreter exists, because every later
//! decision — what the interface offers, what runs, what is refused — hangs
//! off the answer, and the failure that matters is answering "runnable"
//! about something that isn't. A script half-applied is worse than one
//! refused: the user believes a cheat is on when it isn't.
//!
//! ## What Cheat Engine actually does
//!
//! Read out of `autoassembler.pas` rather than inferred from the syntax,
//! because `{$LUA}` does not mean what the name suggests:
//!
//! - **`{$LUA}` is a block directive, not a script-type marker.** It opens a
//!   block that runs until `{$ASM}` or the end of the script, and a script
//!   may contain several, interleaved with assembly.
//! - **The `[ENABLE]`/`[DISABLE]` split happens first, then the Lua pass.**
//!   The outer `autoassemble` strips the script to the requested half and
//!   only then does `autoassemble2` run the Lua blocks. Doing it the other
//!   way round would execute *both* halves — against a live process, with
//!   side effects. [`parse_script`] follows CE's order for exactly this reason.
//! - **A line outside every section belongs to both halves.** This is what
//!   makes the near-universal `{$lua}` / `if syntaxcheck then return end`
//!   preamble work: it sits above `[ENABLE]`, so both halves get it.
//! - **A block is called with two arguments and one result is read back:**
//!   `lua_pcall(L, 2, 1, 0)`, the arguments being a syntax-check flag and
//!   the entry's memory record. **The result is substituted into the script
//!   as assembly only if it is a string.** A block returning nil
//!   contributes no code.
//! - **More than one `[ENABLE]`, or more than one `[DISABLE]`, is an error
//!   in CE** rather than a merge, so it is an error here too.

/// One piece of a script half, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// The body of a `{$LUA}` block, without the directive lines.
    Lua(String),
    /// Auto Assembler text: everything that isn't inside a `{$LUA}` block.
    Assembler(String),
}

/// What Ferrite could do with a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    /// Nothing to run: no code in either half.
    Empty,
    /// Entirely `{$LUA}` blocks, none of which appears to return a value.
    /// The only kind a data-only interpreter could ever run.
    DataOnlyLua,
    /// Has a `{$LUA}` block that appears to return a value — which Cheat
    /// Engine would substitute into the script as assembly if it were a
    /// string.
    ///
    /// Refused rather than attempted. Running the Lua and then discovering
    /// it produced assembly we can't assemble would leave the target
    /// half-modified, which is the specific outcome this module exists to
    /// prevent.
    GenerativeLua,
    /// Contains Auto Assembler code. Running this needs an assembler,
    /// allocation inside the target and code injection — none of which
    /// Ferrite does.
    Assembler,
}

impl ScriptKind {
    /// Whether a data-only Lua interpreter could run this.
    pub fn is_runnable(self) -> bool {
        matches!(self, Self::DataOnlyLua)
    }

    /// A one-line explanation, phrased for the import report.
    ///
    /// The generative wording says "appears to" on purpose: the check is a
    /// conservative source scan, not a Lua evaluation — see
    /// [`lua_returns_a_value`].
    pub fn reason(self) -> &'static str {
        match self {
            Self::Empty => "the script is empty",
            Self::DataOnlyLua => "a data-only Lua script",
            Self::GenerativeLua => {
                "a Lua script that appears to generate assembly — Ferrite does not assemble or \
                 inject code, and running only its Lua half would leave the target partly modified"
            }
            Self::Assembler => {
                "an Auto Assembler script — running it would mean assembling code, allocating \
                 memory inside the target and patching its execution, none of which Ferrite does"
            }
        }
    }
}

/// Why a script couldn't be parsed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptError {
    /// Cheat Engine rejects a script with two `[ENABLE]` or two `[DISABLE]`
    /// sections rather than merging them, so this does too — inventing a
    /// merge would run code CE never would.
    DuplicateSection(Section),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateSection(section) => {
                write!(f, "more than one {} section", section.marker())
            }
        }
    }
}

impl std::error::Error for ScriptError {}

/// Which half of a script a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Enable,
    Disable,
}

impl Section {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Enable => "[ENABLE]",
            Self::Disable => "[DISABLE]",
        }
    }
}

/// A parsed `<AssemblerScript>`, split into its two halves.
///
/// Both halves are built even when the script has no sections at all: in
/// that case CE treats the whole script as belonging to both, so `enable`
/// and `disable` are identical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub enable: Vec<Block>,
    pub disable: Vec<Block>,
}

impl Script {
    /// Classifies the script. Takes the more restrictive of the two halves:
    /// an entry is only runnable if *both* enabling and disabling it are,
    /// because an entry that can be switched on and not off is a worse
    /// offer than one that was never switched on.
    pub fn kind(&self) -> ScriptKind {
        let (a, b) = (classify_half(&self.enable), classify_half(&self.disable));
        // Ordered most to least restrictive.
        for kind in [
            ScriptKind::Assembler,
            ScriptKind::GenerativeLua,
            ScriptKind::DataOnlyLua,
        ] {
            if a == kind || b == kind {
                return kind;
            }
        }
        ScriptKind::Empty
    }

    /// The Lua bodies of one half, in source order — what an interpreter
    /// would run, once there is one.
    pub fn lua_blocks(&self, section: Section) -> Vec<&str> {
        let half = match section {
            Section::Enable => &self.enable,
            Section::Disable => &self.disable,
        };
        half.iter()
            .filter_map(|b| match b {
                Block::Lua(src) => Some(src.as_str()),
                Block::Assembler(_) => None,
            })
            .collect()
    }
}

fn classify_half(blocks: &[Block]) -> ScriptKind {
    let mut kind = ScriptKind::Empty;
    for block in blocks {
        match block {
            Block::Assembler(text) if !assembler_is_blank(text) => return ScriptKind::Assembler,
            Block::Assembler(_) => {}
            Block::Lua(src) if lua_returns_a_value(src) => kind = ScriptKind::GenerativeLua,
            Block::Lua(src) if !lua_is_blank(src) => {
                if kind != ScriptKind::GenerativeLua {
                    kind = ScriptKind::DataOnlyLua;
                }
            }
            Block::Lua(_) => {}
        }
    }
    kind
}

/// Parses a `<AssemblerScript>` body.
///
/// Follows Cheat Engine's own order: split on `[ENABLE]`/`[DISABLE]` first,
/// then split each half into `{$LUA}` and assembler blocks.
pub fn parse_script(text: &str) -> Result<Script, ScriptError> {
    let mut enable_lines: Vec<&str> = Vec::new();
    let mut disable_lines: Vec<&str> = Vec::new();
    let mut current: Option<Section> = None;
    let (mut seen_enable, mut seen_disable) = (false, false);

    for line in text.lines() {
        match section_marker(line) {
            Some(Section::Enable) => {
                if seen_enable {
                    return Err(ScriptError::DuplicateSection(Section::Enable));
                }
                seen_enable = true;
                current = Some(Section::Enable);
            }
            Some(Section::Disable) => {
                if seen_disable {
                    return Err(ScriptError::DuplicateSection(Section::Disable));
                }
                seen_disable = true;
                current = Some(Section::Disable);
            }
            // A line outside every section belongs to both halves — which is
            // what makes the usual `{$lua}` preamble above `[ENABLE]` apply
            // to enabling *and* disabling.
            None => match current {
                Some(Section::Enable) => enable_lines.push(line),
                Some(Section::Disable) => disable_lines.push(line),
                None => {
                    enable_lines.push(line);
                    disable_lines.push(line);
                }
            },
        }
    }

    Ok(Script {
        enable: split_blocks(&enable_lines),
        disable: split_blocks(&disable_lines),
    })
}

/// Recognises a section marker. CE compares the uppercased, trimmed line
/// against the whole marker, so `  [Enable]  ` is one and `[ENABLE] x` is
/// not.
fn section_marker(line: &str) -> Option<Section> {
    match line.trim().to_ascii_uppercase().as_str() {
        "[ENABLE]" => Some(Section::Enable),
        "[DISABLE]" => Some(Section::Disable),
        _ => None,
    }
}

/// Splits one half's lines into `{$LUA}` and assembler blocks. The `{$LUA}`
/// and `{$ASM}` directive lines are consumed rather than kept, matching
/// CE — it blanks them before assembling.
fn split_blocks(lines: &[&str]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut buffer: Vec<&str> = Vec::new();
    let mut in_lua = false;

    let flush = |blocks: &mut Vec<Block>, buffer: &mut Vec<&str>, in_lua: bool| {
        if buffer.is_empty() {
            return;
        }
        let text = buffer.join("\n");
        buffer.clear();
        blocks.push(if in_lua {
            Block::Lua(text)
        } else {
            Block::Assembler(text)
        });
    };

    for line in lines {
        match line.trim().to_ascii_uppercase().as_str() {
            "{$LUA}" => {
                flush(&mut blocks, &mut buffer, in_lua);
                in_lua = true;
            }
            // `{$ASM}` closes a Lua block. Outside one it is a no-op, which
            // is how CE treats it too.
            "{$ASM}" => {
                flush(&mut blocks, &mut buffer, in_lua);
                in_lua = false;
            }
            _ => buffer.push(line),
        }
    }
    flush(&mut blocks, &mut buffer, in_lua);
    blocks
}

/// Whether Auto Assembler text holds anything that would be assembled,
/// ignoring blank lines and comments. AA takes `//` to end of line and
/// `{...}` as a block comment — but not `{$...}` directives, which are
/// meaningful.
fn assembler_is_blank(text: &str) -> bool {
    let mut rest = text;
    let mut stripped = String::with_capacity(text.len());
    while let Some(open) = rest.find('{') {
        // A `{$...}` directive is code, not a comment.
        if rest[open..].starts_with("{$") {
            let after = open + 2;
            stripped.push_str(&rest[..after]);
            rest = &rest[after..];
            continue;
        }
        stripped.push_str(&rest[..open]);
        match rest[open..].find('}') {
            Some(close) => rest = &rest[open + close + 1..],
            None => {
                rest = ""; // unterminated block comment swallows the rest
                break;
            }
        }
    }
    stripped.push_str(rest);

    stripped.lines().all(|line| {
        let line = line.trim();
        line.is_empty() || line.starts_with("//")
    })
}

/// Whether Lua source has any statements at all, ignoring comments.
fn lua_is_blank(src: &str) -> bool {
    strip_lua_comments_and_strings(src)
        .split_whitespace()
        .next()
        .is_none()
}

/// Whether a Lua block appears to `return` a value.
///
/// **Deliberately conservative, and only approximately right.** Deciding
/// this exactly would need a Lua parser; this is a source scan, so it can
/// report a value-returning block that never actually returns one on the
/// path taken. That direction is the safe one: a false "generative" refuses
/// a script Ferrite might have run, while a false "data-only" runs half a
/// cheat. Once an interpreter exists the precise check is available for
/// free — the block's actual result — and this stays as the pre-flight.
///
/// A **bare** `return` is not a value. That distinction is what makes the
/// check usable at all: `if syntaxcheck then return end` is the standard
/// preamble of nearly every real Cheat Engine Lua script, and treating any
/// `return` as generative would refuse essentially all of them.
fn lua_returns_a_value(src: &str) -> bool {
    let code = strip_lua_comments_and_strings(src);
    let bytes = code.as_bytes();
    let mut i = 0;
    while let Some(found) = code[i..].find("return") {
        let start = i + found;
        let end = start + "return".len();
        i = end;

        // Must be a whole word, not `returned` or `myreturn`.
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_word_byte(bytes[end]);
        if !(before_ok && after_ok) {
            continue;
        }

        // What follows decides whether it is bare. `end`, `else`, `elseif`,
        // `until` and `;` all close the statement; so does end of input.
        let tail = code[end..].trim_start();
        let bare = tail.is_empty()
            || tail.starts_with(';')
            || ["end", "else", "elseif", "until"]
                .iter()
                .any(|kw| tail.starts_with(kw) && !tail[kw.len()..].starts_with(is_word_char));
        if !bare {
            return true;
        }
    }
    false
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Stands in for a string literal once its contents are removed.
///
/// A literal cannot become whitespace: `return 'mov eax,1'` would then read
/// as a bare `return`, and the block would be called data-only when it is
/// generating assembly — the exact misclassification this module exists to
/// avoid. Found by the test, not by reading the code.
const STRING_PLACEHOLDER: &str = "\"s\"";

/// Removes Lua comments and the *contents* of string literals, so a
/// `return` inside either doesn't read as code while a literal still counts
/// as a value. Length is not preserved; only the remaining code matters.
fn strip_lua_comments_and_strings(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;

    while i < b.len() {
        // Long bracket, as a comment (`--[[`) or a string (`[[`). Lua allows
        // any number of `=` between the brackets; the closer must match.
        let long_open = if b[i..].starts_with(b"--[") {
            long_bracket_level(&b[i + 2..]).map(|lvl| (lvl, i + 2, true))
        } else if b[i] == b'[' {
            long_bracket_level(&b[i..]).map(|lvl| (lvl, i, false))
        } else {
            None
        };
        if let Some((level, open_at, is_comment)) = long_open {
            let body = open_at + 2 + level;
            let closer = format!("]{}]", "=".repeat(level));
            i = match src[body..].find(&closer) {
                Some(rel) => body + rel + closer.len(),
                None => b.len(), // unterminated: the rest is comment/string
            };
            out.push_str(if is_comment { " " } else { STRING_PLACEHOLDER });
            continue;
        }

        // Line comment.
        if b[i..].starts_with(b"--") {
            i = match src[i..].find('\n') {
                Some(rel) => i + rel, // keep the newline itself
                None => b.len(),
            };
            out.push(' ');
            continue;
        }

        // Quoted string, honouring backslash escapes.
        if b[i] == b'"' || b[i] == b'\'' {
            let quote = b[i];
            i += 1;
            while i < b.len() && b[i] != quote {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            out.push_str(STRING_PLACEHOLDER);
            continue;
        }

        let ch_len = src[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&src[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// If `b` starts a long bracket (`[[`, `[=[`, `[==[` …), its level — the
/// number of `=` signs.
fn long_bracket_level(b: &[u8]) -> Option<usize> {
    if b.first() != Some(&b'[') {
        return None;
    }
    let level = b[1..].iter().take_while(|&&c| c == b'=').count();
    (b.get(1 + level) == Some(&b'[')).then_some(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_with_no_sections_belongs_to_both_halves() {
        // CE keeps a line that is in no section for both enable and
        // disable, so a bare script is symmetric.
        let script = parse_script("{$lua}\nprint('hi')").expect("parses");
        assert_eq!(script.enable, script.disable);
        assert_eq!(script.enable, vec![Block::Lua("print('hi')".to_string())]);
        assert_eq!(script.kind(), ScriptKind::DataOnlyLua);
    }

    #[test]
    fn sections_split_before_the_lua_pass() {
        // The ordering that matters: `[ENABLE]` is consumed as a section
        // marker even though it sits inside what looks like a Lua block, so
        // each half gets the shared preamble plus its own body - and never
        // the other half's. Getting this backwards would run both.
        let script = parse_script(
            "{$lua}\n\
             if syntaxcheck then return end\n\
             [ENABLE]\n\
             writeInteger(addr, 100)\n\
             [DISABLE]\n\
             writeInteger(addr, 1)",
        )
        .expect("parses");

        assert_eq!(
            script.lua_blocks(Section::Enable),
            vec!["if syntaxcheck then return end\nwriteInteger(addr, 100)"]
        );
        assert_eq!(
            script.lua_blocks(Section::Disable),
            vec!["if syntaxcheck then return end\nwriteInteger(addr, 1)"]
        );
        assert_eq!(script.kind(), ScriptKind::DataOnlyLua);
    }

    #[test]
    fn the_syntaxcheck_preamble_is_not_generative() {
        // `if syntaxcheck then return end` opens nearly every real CE Lua
        // script. Treating a bare `return` as a value would refuse almost
        // all of them, which would make the classifier useless rather than
        // cautious.
        assert!(!lua_returns_a_value("if syntaxcheck then return end"));
        assert!(!lua_returns_a_value("if x then return; end"));
        assert!(!lua_returns_a_value("do return end"));
        assert!(!lua_returns_a_value("return"));
    }

    #[test]
    fn a_value_returning_block_is_generative() {
        // CE substitutes a string result into the script as assembly, so a
        // block that returns one is emitting code.
        assert!(lua_returns_a_value("return 'mov eax,1'"));
        assert!(lua_returns_a_value("return string.format('db %X', 0x90)"));
        assert!(lua_returns_a_value("if x then return 'nop' end"));

        let script = parse_script("{$lua}\nreturn 'mov eax,1'").expect("parses");
        assert_eq!(script.kind(), ScriptKind::GenerativeLua);
        assert!(!script.kind().is_runnable());
    }

    #[test]
    fn return_inside_a_comment_or_string_is_not_a_return() {
        assert!(!lua_returns_a_value("-- return 'nope'"));
        assert!(!lua_returns_a_value("--[[ return 'nope' ]]"));
        assert!(!lua_returns_a_value("--[==[ return 'nope' ]==]"));
        assert!(!lua_returns_a_value("x = 'return 1'"));
        assert!(!lua_returns_a_value("x = \"return 1\""));
        assert!(!lua_returns_a_value("x = [[return 1]]"));
        assert!(!lua_returns_a_value("x = 'it\\'s return'"));
        // ...and a word merely containing it isn't either.
        assert!(!lua_returns_a_value("returned = 1"));
        assert!(!lua_returns_a_value("local myreturn = 2"));
    }

    #[test]
    fn assembly_anywhere_makes_it_an_assembler_script() {
        let script = parse_script(
            "[ENABLE]\n\
             aobscanmodule(hook,game.exe,48 8B 4E 18)\n\
             alloc(newmem,$1000,hook)\n\
             newmem:\n\
               mov [rsi+1C],#100\n\
             [DISABLE]\n\
             dealloc(newmem)",
        )
        .expect("parses");
        assert_eq!(script.kind(), ScriptKind::Assembler);
        assert!(!script.kind().is_runnable());
    }

    #[test]
    fn a_lua_block_next_to_assembly_is_still_an_assembler_script() {
        // The mixed case: CE runs the Lua as a helper and assembles the
        // rest. Ferrite can do the first half and not the second, which is
        // precisely the half-application to refuse.
        let script = parse_script(
            "[ENABLE]\n\
             {$lua}\n\
             print('setting up')\n\
             {$asm}\n\
             mov eax,1",
        )
        .expect("parses");
        assert_eq!(script.kind(), ScriptKind::Assembler);
    }

    #[test]
    fn assembler_comments_and_blank_lines_are_not_code() {
        assert!(assembler_is_blank(""));
        assert!(assembler_is_blank("\n  \n"));
        assert!(assembler_is_blank("// just a note\n// another"));
        assert!(assembler_is_blank("{ a block comment }"));
        assert!(assembler_is_blank("{ unterminated"));
        assert!(!assembler_is_blank("mov eax,1"));
        assert!(!assembler_is_blank("// note\nmov eax,1"));
        // A `{$...}` directive is meaningful, not a comment.
        assert!(!assembler_is_blank("{$try}"));
    }

    #[test]
    fn an_asm_directive_closes_a_lua_block() {
        let script =
            parse_script("{$lua}\nprint(1)\n{$asm}\n// nothing\n{$lua}\nprint(2)").expect("parses");
        assert_eq!(
            script.lua_blocks(Section::Enable),
            vec!["print(1)", "print(2)"]
        );
        // The assembler block in the middle is comments only, so this stays
        // runnable rather than being called an assembler script.
        assert_eq!(script.kind(), ScriptKind::DataOnlyLua);
    }

    #[test]
    fn duplicate_sections_are_refused_the_way_cheat_engine_refuses_them() {
        assert_eq!(
            parse_script("[ENABLE]\nx\n[ENABLE]\ny"),
            Err(ScriptError::DuplicateSection(Section::Enable))
        );
        assert_eq!(
            parse_script("[DISABLE]\nx\n[DISABLE]\ny"),
            Err(ScriptError::DuplicateSection(Section::Disable))
        );
        // One of each is the normal shape.
        assert!(parse_script("[ENABLE]\nx\n[DISABLE]\ny").is_ok());
    }

    #[test]
    fn a_section_marker_is_the_whole_line_case_insensitively() {
        let script = parse_script("  [Enable]  \nprint(1)").expect("parses");
        assert_eq!(script.enable.len(), 1);
        assert!(script.disable.is_empty(), "the disable half has no lines");

        // Not a marker: something else on the line.
        let script = parse_script("[ENABLE] extra\nmov eax,1").expect("parses");
        assert_eq!(script.kind(), ScriptKind::Assembler);
    }

    #[test]
    fn an_empty_script_is_empty_not_runnable() {
        for text in ["", "\n\n", "// only a comment", "{$lua}\n-- nothing"] {
            assert_eq!(
                parse_script(text).expect("parses").kind(),
                ScriptKind::Empty,
                "expected {text:?} to be Empty"
            );
        }
        assert!(!ScriptKind::Empty.is_runnable());
    }

    #[test]
    fn one_half_being_unrunnable_makes_the_whole_entry_unrunnable() {
        // An entry that can be switched on but not off is a worse offer
        // than one that was never switched on, so the stricter half wins.
        let script = parse_script(
            "[ENABLE]\n\
             {$lua}\n\
             writeInteger(addr, 100)\n\
             [DISABLE]\n\
             mov [rsi+1C],eax",
        )
        .expect("parses");
        assert_eq!(script.enable.len(), 1);
        assert_eq!(script.kind(), ScriptKind::Assembler);
    }
}
