//! Functions of the data packs as loaded, like the game: continuations joined, comments and
//! empty lines dropped, every plain line checked at the function permission level (2) and its
//! level-2 parse kept once made, macro lines kept as templates, and the last instantiations of a
//! macro function kept by argument values.

use std::sync::{Arc, OnceLock};

use super::paths::CommandPaths;
use super::{Level, Parsed};

/// The kept parse of a line: made at the line's first run (the load only checks the line, like
/// before: no load time or memory for lines that never run), then reused by every run; shared by
/// the identical lines of one load.
pub type KeptParse = Arc<OnceLock<Arc<Parsed>>>;

/// A command line ready to run: its text (traces of `/debug function`, lines run by text) and
/// its kept parse (`None`: parsed again at every run).
#[derive(Debug, Clone)]
pub struct ReadyLine {
    pub text: Arc<str>,
    pub parsed: Option<KeptParse>,
}

impl ReadyLine {
    fn text_only(text: &str) -> ReadyLine {
        ReadyLine { text: Arc::from(text), parsed: None }
    }

    /// A line whose parse is already made (macro instantiation).
    fn parsed_now(text: String, parsed: Parsed) -> ReadyLine {
        ReadyLine { text: Arc::from(text), parsed: Some(Arc::new(OnceLock::from(Arc::new(parsed)))) }
    }
}

/// One line of a loaded macro function.
#[derive(Debug)]
pub enum FunctionLine {
    /// Plain command.
    Command(ReadyLine),
    /// Macro line (boxed: rare, and the plain lines stay small).
    Macro(Box<MacroLine>),
}

/// Macro line: `text` as written (with its `$`), substituted then parsed at each instantiation;
/// `params` gives, for each variable of the template, its index among the function's parameters.
#[derive(Debug)]
pub struct MacroLine {
    text: Arc<str>,
    template: Template,
    params: Vec<usize>,
}

impl FunctionLine {
    pub fn text(&self) -> &str {
        match self {
            FunctionLine::Command(l) => &l.text,
            FunctionLine::Macro(m) => &m.text,
        }
    }
}

/// Number of instantiations a macro function keeps, like the game.
const KEPT_INSTANCES: usize = 8;

/// A compiled function.
#[derive(Debug)]
pub struct Function {
    /// Macro function: all its lines; empty for a plain function.
    lines: Vec<FunctionLine>,
    /// Macro parameters in order of first appearance (the game reports the first one missing);
    /// empty for a plain function.
    pub params: Vec<String>,
    /// Plain function: its lines, shared by every call; empty for a macro function.
    ready: Arc<[ReadyLine]>,
    /// Last successful instantiations (argument values → lines), most recent last.
    instances: Vec<(Vec<String>, Arc<[ReadyLine]>)>,
}

impl Function {
    pub fn is_macro(&self) -> bool {
        !self.params.is_empty()
    }

    /// Texts of the lines, macro lines with their `$`.
    pub fn line_texts(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        if self.is_macro() {
            Box::new(self.lines.iter().map(FunctionLine::text))
        } else {
            Box::new(self.ready.iter().map(|l| &*l.text))
        }
    }

    /// Lines of a plain function as kept (benches: memory).
    #[cfg(test)]
    pub(crate) fn ready_lines(&self) -> &[ReadyLine] {
        &self.ready
    }

    /// Every kept parse made, as after one run of each line (benches: memory).
    #[cfg(test)]
    pub(crate) fn parse_every_line(&self, lvl: &Level, data: &rc_commands::Data) {
        for line in self.ready.iter() {
            if let Some(cell) = &line.parsed {
                cell.get_or_init(|| Arc::new(super::parse_function_line(lvl, data, &line.text, true).expect("checked line")));
            }
        }
    }

    /// Lines of a plain function: shared (`FUNCTIONCACHE`), or copied by text like before (each
    /// line then parsed again when it runs).
    pub fn plain_lines(&self, paths: &CommandPaths) -> Arc<[ReadyLine]> {
        if paths.function_cache {
            return self.ready.clone();
        }
        self.line_texts().map(ReadyLine::text_only).collect()
    }

    /// Instantiation kept for these argument values, now the most recent one.
    pub fn kept_instance(&mut self, values: &[String]) -> Option<Arc<[ReadyLine]>> {
        let at = self.instances.iter().position(|(v, _)| v == values)?;
        let hit = self.instances.remove(at);
        let lines = hit.1.clone();
        self.instances.push(hit);
        Some(lines)
    }

    /// Keeps a successful instantiation, forgetting the oldest beyond 8.
    pub fn keep_instance(&mut self, values: Vec<String>, lines: Arc<[ReadyLine]>) {
        if self.instances.len() >= KEPT_INSTANCES {
            self.instances.remove(0);
        }
        self.instances.push((values, lines));
    }

    /// Lines of a macro function for `values` (in parameter order): macro lines substituted and
    /// checked at level 2; error = (substituted command, the parser's message).
    ///
    /// `MACROCACHE`: one parse per macro line gives the message and the command run, and the
    /// plain lines share their own. Otherwise the check's parse is thrown away and every line is
    /// parsed again when it runs, like before.
    pub fn instantiate(
        &self,
        values: &[String],
        lvl: Option<&Level>,
        data: &rc_commands::Data,
        paths: &CommandPaths,
    ) -> Result<Arc<[ReadyLine]>, (String, String)> {
        let mut out = Vec::with_capacity(self.lines.len());
        for line in &self.lines {
            let FunctionLine::Macro(m) = line else {
                out.push(match line {
                    FunctionLine::Command(l) if paths.macro_cache => l.clone(),
                    other => ReadyLine::text_only(other.text()),
                });
                continue;
            };
            let command = m.template.substitute(m.params.iter().map(|&i| values[i].as_str()));
            let Some(lvl) = lvl else {
                out.push(ReadyLine::text_only(&command));
                continue;
            };
            if paths.macro_cache {
                match super::parse_function_line(lvl, data, &command, paths.parser_no_alloc) {
                    Ok(p) => out.push(ReadyLine::parsed_now(command, p)),
                    Err(msg) => return Err((command, msg)),
                }
            } else {
                if let Some(msg) = super::syntax_message(lvl, data, &command, 2, paths.parser_no_alloc) {
                    return Err((command, msg));
                }
                out.push(ReadyLine::text_only(&command));
            }
        }
        Ok(out.into())
    }
}

/// Spaces removed like Java's `String.trim` (characters ≤ U+0020).
fn jtrim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

/// Compilation of a function at load time, like the game: `\` continuations joined, comments
/// and empty lines removed, macro lines (`$`) kept as templates; any invalid line (command that
/// does not parse at level 2, leading `/`, macro without a variable…) makes the whole function
/// refused (the game's log text). The parse of each plain line is kept (`FUNCTIONCACHE`).
pub fn compile_function(raw: &[&str], lvl: Option<&Level>, data: &rc_commands::Data) -> Result<Function, String> {
    compile_with(raw, lvl, data, &CommandPaths::from_switches(), &mut SharedLines::default())
}

/// Lines already checked during one load, by text: identical lines (frequent in generated
/// packs) are checked once and share their text and kept parse, the parse being a pure function
/// of the text and the pack data.
#[derive(Default)]
pub(crate) struct SharedLines(hashbrown::HashMap<Arc<str>, KeptParse>);

impl SharedLines {
    fn line(&mut self, text: String, lvl: &Level, data: &rc_commands::Data, paths: &CommandPaths) -> Result<ReadyLine, String> {
        if let Some((text, parsed)) = self.0.get_key_value(text.as_str()) {
            return Ok(ReadyLine { text: text.clone(), parsed: Some(parsed.clone()) });
        }
        if let Some(msg) = super::syntax_message(lvl, data, &text, 2, paths.parser_no_alloc) {
            return Err(msg);
        }
        let text: Arc<str> = Arc::from(text);
        let parsed = KeptParse::default();
        self.0.insert(text.clone(), parsed.clone());
        Ok(ReadyLine { text, parsed: Some(parsed) })
    }
}

pub(crate) fn compile_with(
    raw: &[&str],
    lvl: Option<&Level>,
    data: &rc_commands::Data,
    paths: &CommandPaths,
    shared: &mut SharedLines,
) -> Result<Function, String> {
    let mut lines = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let number = i + 1;
        let mut line = jtrim(raw[i]).to_owned();
        while line.ends_with('\\') {
            i += 1;
            if i == raw.len() {
                return Err("Line continuation at end of file".into());
            }
            line.pop();
            line.push_str(jtrim(raw[i]));
        }
        i += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('/') {
            return Err(slash_error(&line, rest, number));
        }
        if let Some(m) = line.strip_prefix('$') {
            let Ok(template) = Template::parse(m) else {
                return Err(format!("Can't parse function line {number}: '{m}'"));
            };
            let indices = template.vars.iter().map(|v| parameter_index(&mut params, v)).collect();
            lines.push(FunctionLine::Macro(Box::new(MacroLine { text: Arc::from(line), template, params: indices })));
            continue;
        }
        let Some(lvl) = lvl else {
            lines.push(FunctionLine::Command(ReadyLine::text_only(&line)));
            continue;
        };
        let ready = if paths.function_cache {
            shared.line(line, lvl, data, paths)
        } else {
            // Without the cache, the line is only checked and its text kept (the former way).
            match super::syntax_message(lvl, data, &line, 2, paths.parser_no_alloc) {
                Some(msg) => Err(msg),
                None => Ok(ReadyLine::text_only(&line)),
            }
        };
        match ready {
            Ok(r) => lines.push(FunctionLine::Command(r)),
            Err(msg) => return Err(format!("Whilst parsing command on line {number}: {msg}")),
        }
    }
    if !params.is_empty() {
        lines.shrink_to_fit();
        return Ok(Function { lines, params, ready: Arc::from(Vec::new()), instances: Vec::new() });
    }
    // Plain function: only the shared list of ready lines is kept.
    let ready = lines
        .into_iter()
        .filter_map(|l| match l {
            FunctionLine::Command(r) => Some(r),
            FunctionLine::Macro(_) => None,
        })
        .collect();
    Ok(Function { lines: Vec::new(), params, ready, instances: Vec::new() })
}

/// The game's message for a line starting with `/`.
fn slash_error(line: &str, rest: &str, number: usize) -> String {
    if rest.starts_with('/') {
        return format!(
            "Unknown or invalid command '{line}' on line {number} (if you intended to make a comment, use '#' not '//')"
        );
    }
    let name: String = rest.chars().take_while(|&c| rc_commands::reader::is_unquoted(c)).collect();
    format!("Unknown or invalid command '{line}' on line {number} (did you mean '{name}'? Do not use a preceding forwards slash.)")
}

/// Index of parameter `name`, added at the end on its first appearance.
fn parameter_index(params: &mut Vec<String>, name: &str) -> usize {
    match params.iter().position(|p| p == name) {
        Some(i) => i,
        None => {
            params.push(name.to_owned());
            params.len() - 1
        }
    }
}

/// Template of a macro line: pieces of text and interspersed `$(name)` variables.
#[derive(Debug)]
pub struct Template {
    pub segments: Vec<String>,
    pub vars: Vec<String>,
}

impl Template {
    pub fn parse(input: &str) -> Result<Template, &'static str> {
        let b = input.as_bytes();
        let (mut segments, mut vars) = (Vec::new(), Vec::new());
        let mut start = 0;
        let mut index = input.find('$');
        while let Some(at) = index {
            if at != b.len() - 1 && b[at + 1] == b'(' {
                segments.push(input[start..at].to_owned());
                let Some(end) = input[at + 1..].find(')').map(|e| e + at + 1) else {
                    return Err("Unterminated macro variable");
                };
                let var = &input[at + 2..end];
                if !var.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    return Err("Invalid macro variable name");
                }
                vars.push(var.to_owned());
                start = end + 1;
                index = input[start..].find('$').map(|e| e + start);
            } else {
                index = input[at + 1..].find('$').map(|e| e + at + 1);
            }
        }
        if start == 0 {
            return Err("No variables in macro");
        }
        if start != input.len() {
            segments.push(input[start..].to_owned());
        }
        Ok(Template { segments, vars })
    }

    /// The command with `values`, one per variable of the template in order.
    pub fn substitute<'v>(&self, values: impl IntoIterator<Item = &'v str>) -> String {
        let mut s = String::new();
        for (segment, value) in self.segments.iter().zip(values) {
            s.push_str(segment);
            s.push_str(value);
        }
        if self.segments.len() > self.vars.len()
            && let Some(last) = self.segments.last()
        {
            s.push_str(last);
        }
        s
    }
}
