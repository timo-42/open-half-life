//! A Quake-style developer console: scrollback, an input line with history
//! and tab completion, a command registry and a cvar table.

mod buffer;
mod registry;
mod variables;
mod view;

pub use buffer::{MAX_LINES, ScrollbackBuffer};
pub use registry::{CommandContext, CommandError, CommandRegistry, ConsoleEvent, tokenize};
pub use variables::{VarBounds, VarValue, VariableError, Variables};
pub use view::draw as draw_console;

/// The key that toggles the console, matching the Quake-family convention.
pub const TOGGLE_KEY: egui::Key = egui::Key::Backtick;

/// Maximum number of entries [`Console`]'s input history retains. Mirrors
/// [`buffer::MAX_LINES`]'s bound on the scrollback buffer: older entries are
/// dropped first once this is reached.
pub const MAX_HISTORY: usize = 256;

/// The developer console: state plus the built-in commands every instance
/// registers (`help`, `echo`, `set`, `quit`, `map`).
pub struct Console {
    buffer: ScrollbackBuffer,
    registry: CommandRegistry,
    variables: Variables,
    input: String,
    history: Vec<String>,
    history_cursor: Option<usize>,
    pending_events: Vec<ConsoleEvent>,
    open: bool,
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Console {
    /// Creates a console with the built-in commands registered and no
    /// variables defined; callers add their own with [`Self::variables_mut`].
    #[must_use]
    pub fn new() -> Self {
        let mut registry = CommandRegistry::new();
        register_builtins(&mut registry);
        Self {
            buffer: ScrollbackBuffer::new(),
            registry,
            variables: Variables::new(),
            input: String::new(),
            history: Vec::new(),
            history_cursor: None,
            pending_events: Vec::new(),
            open: false,
        }
    }

    /// Whether the console is currently shown.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Shows or hides the console.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// Flips the console's visibility, as the toggle key does.
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// The scrollback buffer, for read access (rendering, tests).
    #[must_use]
    pub fn buffer(&self) -> &ScrollbackBuffer {
        &self.buffer
    }

    /// The command registry, for read access (tab completion, tests).
    #[must_use]
    pub fn registry(&self) -> &CommandRegistry {
        &self.registry
    }

    /// Mutable access to the variable table, so the host can register
    /// additional cvars (sensitivity, volume, fov, ...).
    pub fn variables_mut(&mut self) -> &mut Variables {
        &mut self.variables
    }

    /// Read access to the variable table.
    #[must_use]
    pub fn variables(&self) -> &Variables {
        &self.variables
    }

    /// Registers an additional command, beyond the built-ins.
    pub fn register_command(
        &mut self,
        name: &str,
        handler: impl Fn(&[String], &mut CommandContext<'_>) + Send + Sync + 'static,
    ) -> Result<(), CommandError> {
        self.registry.register(name, handler)
    }

    /// Runs `line` as-is (bypassing the input widget and history), pushing
    /// the echoed input and any error into the scrollback buffer. Returns
    /// any events the command raised.
    pub fn submit_line(&mut self, line: &str) -> Vec<ConsoleEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        // Dedupe immediate repeats (repeatedly pressing enter on the same
        // line, or replaying the last history entry, should not bloat the
        // history), then bound it the same way the scrollback buffer is
        // bounded: drop the oldest entry once the cap is reached.
        if self.history.last().map(String::as_str) != Some(trimmed) {
            self.history.push(trimmed.to_string());
            while self.history.len() > MAX_HISTORY {
                self.history.remove(0);
            }
        }
        self.history_cursor = None;
        self.buffer.push(&format!("] {trimmed}"));

        let mut events = Vec::new();
        {
            let mut context = CommandContext {
                variables: &mut self.variables,
                output: &mut self.buffer,
                events: &mut events,
            };
            if let Err(error) = self.registry.execute(trimmed, &mut context) {
                self.buffer.push(&error.to_string());
            }
        }
        self.pending_events.extend(events.iter().cloned());
        events
    }

    /// Submits the current input line as if the player pressed enter, then
    /// clears it.
    pub fn submit_input(&mut self) -> Vec<ConsoleEvent> {
        let line = std::mem::take(&mut self.input);
        self.submit_line(&line)
    }

    /// The current, not-yet-submitted input line.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Mutable access to the input line, for the text-edit widget.
    pub fn input_mut(&mut self) -> &mut String {
        &mut self.input
    }

    /// Replaces the input line with the previous history entry, if any.
    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next_index = match self.history_cursor {
            Some(0) => 0,
            Some(index) => index - 1,
            None => self.history.len() - 1,
        };
        self.history_cursor = Some(next_index);
        self.input = self.history[next_index].clone();
    }

    /// Replaces the input line with the next, more recent history entry, or
    /// clears it once the newest entry is passed.
    pub fn history_next(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 < self.history.len() {
            self.history_cursor = Some(index + 1);
            self.input = self.history[index + 1].clone();
        } else {
            self.history_cursor = None;
            self.input.clear();
        }
    }

    /// Command names that complete the current input's first word. Returns
    /// an empty vector once a space has been typed (only the command name is
    /// completed, not arguments).
    #[must_use]
    pub fn tab_completions(&self) -> Vec<String> {
        if self.input.contains(char::is_whitespace) {
            return Vec::new();
        }
        self.registry
            .complete(&self.input)
            .map(str::to_string)
            .collect()
    }

    /// Applies tab completion: if exactly one command matches the current
    /// input, replaces the input with it.
    pub fn apply_tab_completion(&mut self) {
        let mut matches = self.tab_completions();
        if matches.len() == 1 {
            self.input = matches.remove(0);
        }
    }

    /// Drains every event raised by commands since the last call.
    pub fn take_events(&mut self) -> Vec<ConsoleEvent> {
        std::mem::take(&mut self.pending_events)
    }
}

fn register_builtins(registry: &mut CommandRegistry) {
    registry
        .register("help", |_, ctx| {
            for name in ctx
                .variables
                .names()
                .map(str::to_string)
                .collect::<Vec<_>>()
            {
                ctx.output.push(&format!("var  {name}"));
            }
        })
        .expect("built-in commands are registered once");
    registry
        .register("echo", |args, ctx| {
            ctx.output.push(&args.join(" "));
        })
        .expect("built-in commands are registered once");
    registry
        .register("set", |args, ctx| match args {
            [name, value] => {
                if let Err(error) = ctx.variables.set(name, value) {
                    ctx.output.push(&error.to_string());
                }
            }
            [name] => match ctx.variables.get(name) {
                Ok(value) => ctx.output.push(&format!("{name} = {value}")),
                Err(error) => ctx.output.push(&error.to_string()),
            },
            _ => ctx.output.push("usage: set <name> [value]"),
        })
        .expect("built-in commands are registered once");
    registry
        .register("quit", |_, ctx| {
            ctx.events.push(ConsoleEvent::Quit);
        })
        .expect("built-in commands are registered once");
    registry
        .register("map", |args, ctx| {
            if let Some(name) = args.first() {
                ctx.events.push(ConsoleEvent::Map(name.clone()));
            } else {
                ctx.output.push("usage: map <name>");
            }
        })
        .expect("built-in commands are registered once");
}

#[cfg(test)]
mod tests {
    use super::{Console, ConsoleEvent};

    #[test]
    fn help_lists_registered_variables() {
        let mut console = Console::new();
        console
            .variables_mut()
            .register_bool("dev.flag", true)
            .unwrap();
        console.submit_line("help");
        assert!(
            console
                .buffer()
                .lines()
                .any(|line| line.contains("dev.flag"))
        );
    }

    #[test]
    fn echo_appends_its_arguments() {
        let mut console = Console::new();
        console.submit_line("echo hello world");
        assert!(console.buffer().lines().any(|line| line == "hello world"));
    }

    #[test]
    fn set_reads_and_writes_variables() {
        let mut console = Console::new();
        console
            .variables_mut()
            .register_int("sv.foo", 1, super::VarBounds::UNBOUNDED)
            .unwrap();
        console.submit_line("set sv.foo 5");
        assert_eq!(
            console.variables().get("sv.foo").unwrap(),
            &super::VarValue::Int(5)
        );
    }

    #[test]
    fn quit_raises_a_quit_event() {
        let mut console = Console::new();
        let events = console.submit_line("quit");
        assert_eq!(events, vec![ConsoleEvent::Quit]);
    }

    #[test]
    fn map_raises_a_map_event_carrying_the_name() {
        let mut console = Console::new();
        let events = console.submit_line("map c1a0");
        assert_eq!(events, vec![ConsoleEvent::Map("c1a0".to_string())]);
    }

    #[test]
    fn take_events_drains_accumulated_events() {
        let mut console = Console::new();
        console.submit_line("quit");
        assert_eq!(console.take_events(), vec![ConsoleEvent::Quit]);
        assert!(console.take_events().is_empty());
    }

    #[test]
    fn history_navigation_cycles_through_past_lines() {
        let mut console = Console::new();
        console.submit_line("echo one");
        console.submit_line("echo two");
        console.history_previous();
        assert_eq!(console.input(), "echo two");
        console.history_previous();
        assert_eq!(console.input(), "echo one");
        console.history_next();
        assert_eq!(console.input(), "echo two");
        console.history_next();
        assert_eq!(console.input(), "");
    }

    #[test]
    fn tab_completion_fills_a_single_match() {
        let mut console = Console::new();
        *console.input_mut() = "he".to_string();
        console.apply_tab_completion();
        assert_eq!(console.input(), "help");
    }

    #[test]
    fn tab_completion_leaves_ambiguous_input_untouched() {
        let mut console = Console::new();
        console.register_command("mapname", |_, _| {}).unwrap();
        *console.input_mut() = "map".to_string();
        let mut matches = console.tab_completions();
        matches.sort_unstable();
        assert_eq!(matches, vec!["map", "mapname"]);
        console.apply_tab_completion();
        assert_eq!(console.input(), "map");
    }

    #[test]
    fn history_dedupes_consecutive_repeats() {
        let mut console = Console::new();
        console.submit_line("echo one");
        console.submit_line("echo dup");
        console.submit_line("echo dup");
        console.history_previous();
        assert_eq!(console.input(), "echo dup");
        console.history_previous();
        assert_eq!(
            console.input(),
            "echo one",
            "the repeated line must have been recorded only once"
        );
    }

    #[test]
    fn history_is_bounded_and_drops_the_oldest_entries() {
        let mut console = Console::new();
        // Every line is distinct, so none of these get deduped; this pushes
        // MAX_HISTORY + 10 entries through a cap of MAX_HISTORY.
        for index in 0..super::MAX_HISTORY + 10 {
            console.submit_line(&format!("echo {index}"));
        }

        // Walking all the way back must stop after exactly MAX_HISTORY
        // entries, landing on the oldest surviving one.
        for _ in 0..super::MAX_HISTORY {
            console.history_previous();
        }
        assert_eq!(
            console.input(),
            "echo 10",
            "the first 10 entries should have been dropped to respect the cap"
        );
        let oldest = console.input().to_string();
        console.history_previous();
        assert_eq!(
            console.input(),
            oldest,
            "there must be no entry older than the cap allows"
        );
    }

    #[test]
    fn toggle_flips_visibility() {
        let mut console = Console::new();
        assert!(!console.is_open());
        console.toggle();
        assert!(console.is_open());
        console.toggle();
        assert!(!console.is_open());
    }

    #[test]
    fn unknown_command_reports_an_error_in_the_buffer() {
        let mut console = Console::new();
        console.submit_line("bogus");
        assert!(
            console
                .buffer()
                .lines()
                .any(|line| line.contains("unknown command"))
        );
    }
}
