//! Which optimised command paths the server takes, each behind its switch (`=0` restores the
//! former way), read once per process when the world opens.

/// Optimised command paths of one world's command state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandPaths {
    /// `RUSTCRAFT_OPT_FUNCTIONCACHE`: the plain lines of a function keep the parse made when the
    /// pack loads (level 2, like the game) and a call shares the function's lines instead of
    /// copying them; otherwise every line is parsed again at each call.
    pub function_cache: bool,
    /// `RUSTCRAFT_OPT_MACROCACHE`: a macro line is parsed once per instantiation (the same parse
    /// gives the error text and the command run), and the last 8 instantiations of a function
    /// are kept by argument values, like the game.
    pub macro_cache: bool,
    /// `RUSTCRAFT_OPT_SHAREDSOURCE`: a function line runs with a borrowed source instead of two
    /// deep copies of it (one per line, one kept for the fork-limit message).
    pub shared_source: bool,
    /// `RUSTCRAFT_OPT_PARSECACHE`: command blocks, minecarts and typed commands find the parse of
    /// a text they already ran in a bounded cache (emptied at each `/reload`).
    pub parse_cache: bool,
    /// `RUSTCRAFT_OPT_COMMANDBLOCKBORROW`: a command block reads its block entity once per step
    /// and by reference, and recognises its kind by block identifier instead of by name.
    pub command_block_borrow: bool,
    /// `RUSTCRAFT_OPT_LOCALTIMECACHE`: the `[HH:mm:ss]` stamp of a command block's output is
    /// formatted once per second and the time zone offset read once per minute.
    pub local_time_cache: bool,
    /// `RUSTCRAFT_OPT_SCHEDULEQUEUE`: `/schedule` entries ordered by due tick, so a tick with
    /// nothing due looks at one entry instead of splitting the whole list.
    pub schedule_queue: bool,
    /// `RUSTCRAFT_OPT_PARSERNOALLOC`: the command parser tries the tree's nodes without copying
    /// the parse context, the child lists or the words it compares.
    pub parser_no_alloc: bool,
}

#[cfg(test)]
thread_local! {
    /// Tests and benches: paths of the worlds opened by this thread (every combination of the
    /// equivalence proofs), without touching the environment.
    pub(crate) static FORCED_PATHS: std::cell::Cell<Option<CommandPaths>> = const { std::cell::Cell::new(None) };
}

impl CommandPaths {
    pub fn from_switches() -> CommandPaths {
        #[cfg(test)]
        if let Some(paths) = FORCED_PATHS.with(std::cell::Cell::get) {
            return paths;
        }
        CommandPaths {
            function_cache: rc_data::opt_switch!("RUSTCRAFT_OPT_FUNCTIONCACHE"),
            macro_cache: rc_data::opt_switch!("RUSTCRAFT_OPT_MACROCACHE"),
            shared_source: rc_data::opt_switch!("RUSTCRAFT_OPT_SHAREDSOURCE"),
            parse_cache: rc_data::opt_switch!("RUSTCRAFT_OPT_PARSECACHE"),
            command_block_borrow: rc_data::opt_switch!("RUSTCRAFT_OPT_COMMANDBLOCKBORROW"),
            local_time_cache: rc_data::opt_switch!("RUSTCRAFT_OPT_LOCALTIMECACHE"),
            schedule_queue: rc_data::opt_switch!("RUSTCRAFT_OPT_SCHEDULEQUEUE"),
            parser_no_alloc: rc_commands::parse::no_alloc_switch(),
        }
    }

    /// Every path off: the code before the optimisation.
    #[cfg(test)]
    pub const OLD: CommandPaths = CommandPaths {
        function_cache: false,
        macro_cache: false,
        shared_source: false,
        parse_cache: false,
        command_block_borrow: false,
        local_time_cache: false,
        schedule_queue: false,
        parser_no_alloc: false,
    };

    /// Every path on: the defaults.
    #[cfg(test)]
    pub const NEW: CommandPaths = CommandPaths {
        function_cache: true,
        macro_cache: true,
        shared_source: true,
        parse_cache: true,
        command_block_borrow: true,
        local_time_cache: true,
        schedule_queue: true,
        parser_no_alloc: true,
    };

    /// Runs `f` with the worlds it opens on `paths`.
    #[cfg(test)]
    pub(crate) fn forced<T>(paths: CommandPaths, f: impl FnOnce() -> T) -> T {
        let before = FORCED_PATHS.with(|p| p.replace(Some(paths)));
        let out = f();
        FORCED_PATHS.with(|p| p.set(before));
        out
    }
}
