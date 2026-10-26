//! Client-side chat commands (`docs/specs/commands.md`): received tree, parsing of the input
//! (walk and readers of the 62 argument types), suggestions, usage, signable arguments.
//!
//! Positions are character indices in the full input (the `/` included).

pub mod args;
pub mod components;
pub mod data;
pub mod javafmt;
pub mod parse;
pub mod reader;
pub mod selector;
pub mod snbt;
pub mod suggest;
pub mod tree;
pub mod usage;

pub use data::Data;
pub use parse::{Ctx, Parse, parse};
pub use suggest::{Pending, Suggestion, Suggestions, Tooltip};
pub use tree::{NodeId, Tree};

/// Parsing error: translation key, parameters, reader position (context) if known.
#[derive(Debug, Clone, PartialEq)]
pub struct CmdError {
    pub key: &'static str,
    pub args: Vec<String>,
    /// Cursor at the time of the error (`None`: error without context).
    pub cursor: Option<usize>,
}

/// Key of the "incorrect literal" error (filtered separately in the display, § 4.4).
pub const LITERAL_INCORRECT: &str = "argument.literal.incorrect";

impl CmdError {
    pub fn new(key: &'static str, args: Vec<String>, cursor: usize) -> Self {
        CmdError { key, args, cursor: Some(cursor) }
    }

    /// Error context: `...` if truncated, at most 10 characters before the cursor, `<--[HERE]`.
    pub fn context(&self, input: &[char]) -> Option<String> {
        let c = self.cursor?.min(input.len());
        let mut s = String::new();
        if c > 10 {
            s.push_str("...");
        }
        s.extend(&input[c.saturating_sub(10)..c]);
        s.push_str("<--[HERE]");
        Some(s)
    }
}
