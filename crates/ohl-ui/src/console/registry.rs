//! The console's command table: registration, line parsing, execution and
//! tab completion.

use std::collections::BTreeMap;
use std::fmt;

use super::buffer::ScrollbackBuffer;
use super::variables::Variables;

/// An event a command handler can raise for the host application to act on.
/// The console itself never validates a map name against the asset index or
/// tears down the process on `quit`; it only reports intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleEvent {
    /// The `quit` command ran; the host should begin shutdown.
    Quit,
    /// The `map <name>` command ran with `name`; the host should validate it
    /// against the asset index and, if valid, change levels.
    Map(String),
}

/// Everything a command handler is given access to.
pub struct CommandContext<'a> {
    /// The console variable table.
    pub variables: &'a mut Variables,
    /// The scrollback buffer; handlers append their output here.
    pub output: &'a mut ScrollbackBuffer,
    /// Events raised by this invocation are pushed here for the caller of
    /// [`CommandRegistry::execute`] to drain afterwards.
    pub events: &'a mut Vec<ConsoleEvent>,
}

/// A command's fixed, sanitized-safe failure. Argument text a handler
/// chooses to echo into `output` is not part of this type; this only covers
/// the registry's own bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    /// No command is registered under that name.
    NotFound(String),
    /// The command name was empty (an all-whitespace or empty line).
    EmptyLine,
    /// A command is already registered under that name.
    AlreadyRegistered(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "unknown command \"{name}\""),
            Self::EmptyLine => f.write_str("empty command line"),
            Self::AlreadyRegistered(name) => write!(f, "command \"{name}\" is already registered"),
        }
    }
}

impl std::error::Error for CommandError {}

type Handler = Box<dyn Fn(&[String], &mut CommandContext<'_>) + Send + Sync>;

/// A named table of console command handlers.
#[derive(Default)]
pub struct CommandRegistry {
    commands: BTreeMap<String, Handler>,
}

/// Splits a console line into whitespace-separated words, honoring
/// double-quoted segments so a single argument may contain spaces.
#[must_use]
pub fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_current = false;
    for character in line.chars() {
        match character {
            '"' => {
                in_quotes = !in_quotes;
                has_current = true;
            }
            character if character.is_whitespace() && !in_quotes => {
                if has_current {
                    tokens.push(std::mem::take(&mut current));
                    has_current = false;
                }
            }
            character => {
                current.push(character);
                has_current = true;
            }
        }
    }
    if has_current {
        tokens.push(current);
    }
    tokens
}

impl CommandRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `handler` under `name`. Fails if `name` is already taken.
    pub fn register(
        &mut self,
        name: &str,
        handler: impl Fn(&[String], &mut CommandContext<'_>) + Send + Sync + 'static,
    ) -> Result<(), CommandError> {
        if self.commands.contains_key(name) {
            return Err(CommandError::AlreadyRegistered(name.to_string()));
        }
        self.commands.insert(name.to_string(), Box::new(handler));
        Ok(())
    }

    /// Parses and runs `line`. The first token selects the command; the
    /// remainder are passed as arguments.
    pub fn execute(
        &self,
        line: &str,
        context: &mut CommandContext<'_>,
    ) -> Result<(), CommandError> {
        let tokens = tokenize(line);
        let Some((name, args)) = tokens.split_first() else {
            return Err(CommandError::EmptyLine);
        };
        let handler = self
            .commands
            .get(name.as_str())
            .ok_or_else(|| CommandError::NotFound(name.clone()))?;
        handler(args, context);
        Ok(())
    }

    /// Every registered command name, in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.commands.keys().map(String::as_str)
    }

    /// Command names starting with `prefix`, for tab completion. Empty
    /// `prefix` matches everything.
    pub fn complete<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> {
        self.names().filter(move |name| name.starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::super::buffer::ScrollbackBuffer;
    use super::super::variables::Variables;
    use super::{CommandContext, CommandError, CommandRegistry, ConsoleEvent, tokenize};

    fn context<'a>(
        variables: &'a mut Variables,
        output: &'a mut ScrollbackBuffer,
        events: &'a mut Vec<ConsoleEvent>,
    ) -> CommandContext<'a> {
        CommandContext {
            variables,
            output,
            events,
        }
    }

    #[test]
    fn tokenize_splits_on_whitespace_and_honors_quotes() {
        assert_eq!(tokenize("map  c1a0"), vec!["map", "c1a0"]);
        assert_eq!(
            tokenize("echo \"hello world\""),
            vec!["echo", "hello world"]
        );
        assert_eq!(tokenize("   "), Vec::<String>::new());
    }

    #[test]
    fn execute_runs_the_registered_handler_with_its_arguments() {
        let mut registry = CommandRegistry::new();
        registry
            .register("echo", |args, ctx| {
                ctx.output.push(&args.join(" "));
            })
            .unwrap();

        let mut variables = Variables::new();
        let mut output = ScrollbackBuffer::new();
        let mut events = Vec::new();
        registry
            .execute(
                "echo hi there",
                &mut context(&mut variables, &mut output, &mut events),
            )
            .unwrap();
        assert_eq!(output.lines().next(), Some("hi there"));
    }

    #[test]
    fn execute_reports_unknown_commands() {
        let registry = CommandRegistry::new();
        let mut variables = Variables::new();
        let mut output = ScrollbackBuffer::new();
        let mut events = Vec::new();
        let error = registry
            .execute(
                "nope",
                &mut context(&mut variables, &mut output, &mut events),
            )
            .unwrap_err();
        assert_eq!(error, CommandError::NotFound("nope".to_string()));
    }

    #[test]
    fn execute_reports_empty_lines() {
        let registry = CommandRegistry::new();
        let mut variables = Variables::new();
        let mut output = ScrollbackBuffer::new();
        let mut events = Vec::new();
        let error = registry
            .execute(
                "   ",
                &mut context(&mut variables, &mut output, &mut events),
            )
            .unwrap_err();
        assert_eq!(error, CommandError::EmptyLine);
    }

    #[test]
    fn registering_the_same_name_twice_fails() {
        let mut registry = CommandRegistry::new();
        registry.register("help", |_, _| {}).unwrap();
        assert_eq!(
            registry.register("help", |_, _| {}).unwrap_err(),
            CommandError::AlreadyRegistered("help".to_string())
        );
    }

    #[test]
    fn complete_matches_by_prefix() {
        let mut registry = CommandRegistry::new();
        registry.register("map", |_, _| {}).unwrap();
        registry.register("maxplayers", |_, _| {}).unwrap();
        registry.register("quit", |_, _| {}).unwrap();
        let mut matches: Vec<_> = registry.complete("ma").collect();
        matches.sort_unstable();
        assert_eq!(matches, vec!["map", "maxplayers"]);
    }

    #[test]
    fn handler_can_raise_events() {
        let mut registry = CommandRegistry::new();
        registry
            .register("map", |args, ctx| {
                if let Some(name) = args.first() {
                    ctx.events.push(ConsoleEvent::Map(name.clone()));
                }
            })
            .unwrap();
        let mut variables = Variables::new();
        let mut output = ScrollbackBuffer::new();
        let mut events = Vec::new();
        registry
            .execute(
                "map c1a0",
                &mut context(&mut variables, &mut output, &mut events),
            )
            .unwrap();
        assert_eq!(events, vec![ConsoleEvent::Map("c1a0".to_string())]);
    }
}
