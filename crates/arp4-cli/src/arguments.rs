//! Reading the command line for agents. An option a command does not take is
//! accepted, and reported in the response, only where leaving it out cannot
//! change the result; any other mistake is rejected with what the command
//! does take, so that one more attempt is enough.
use crate::cli::{Cli, Command, DocumentCommand, OperationCommand, SpecCommand};
use arp4_cli::workflow::WorkflowCommand;
use clap::{
    CommandFactory, Parser,
    error::{ContextKind, ContextValue, ErrorKind},
};
use serde_json::{Map, Value, json};
use std::ffi::OsString;

/// The options a command may be given without effect: the name, whether it
/// takes a value, and the only value that has none (any value when `None`).
const IGNORABLE: [(&str, bool, Option<&str>); 3] = [
    ("--root", true, None),
    ("--include-hashes", false, None),
    ("--format", true, Some("json")),
];

/// Why an option the command at `path` does not take has no effect there.
fn ignorable(option: &str, path: &str) -> Option<&'static str> {
    match option {
        "--root" if path == "doctor" => Some("doctor describes this executable, not a project"),
        // Every command under documents takes it.
        "--include-hashes" => Some("this command returns no verification hashes"),
        // Reading material comes in the project's format, not JSON.
        "--format" if path != "spec workflow read" => Some("standard output is always JSON"),
        _ => None,
    }
}

/// The command line, with the notes on the options it ignored, or the first
/// error the line has.
pub fn parse(args: &[OsString]) -> Result<(Cli, Vec<Value>), clap::Error> {
    let mut remaining = args.to_vec();
    let mut notes = Vec::new();
    let error = loop {
        let error = match Cli::try_parse_from(&remaining) {
            Ok(mut cli) => match paging(&mut cli) {
                Ok(note) => {
                    notes.extend(note);
                    let path = Chain::of(&remaining).path();
                    record(
                        args,
                        "ignored",
                        notes
                            .iter()
                            .map(|note| format!("{path} {}", note["option"].as_str().unwrap())),
                    );
                    return Ok((cli, notes));
                }
                Err(error) => break error,
            },
            Err(error) => error,
        };
        match drop_ignorable(&error, &mut remaining) {
            Some(note) => notes.push(note),
            None => break error,
        }
    };
    if !matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        let chain = Chain::of(&remaining);
        let key = match unexpected(&error) {
            Some(argument) => format!("{} {argument}", chain.path()),
            None => format!("{} {:?}", chain.path(), error.kind()),
        };
        record(args, "rejected", [key.trim().to_owned()]);
    }
    Err(error)
}

/// Take out of `args` the unknown option of `error`, with its value, when it
/// has no effect on the command; the note says why.
fn drop_ignorable(error: &clap::Error, args: &mut Vec<OsString>) -> Option<Value> {
    // clap names the option alone, also when its value was given with `=`.
    let argument = unexpected(error)?;
    let &(option, takes_value, only) = IGNORABLE.iter().find(|(option, ..)| *option == argument)?;
    let reason = ignorable(option, &Chain::of(args).path())?;
    let (at, inline) = args.iter().enumerate().find_map(|(at, arg)| {
        let arg = arg.to_str()?;
        match arg.strip_prefix(option)? {
            "" => Some((at, None)),
            rest => Some((at, Some(rest.strip_prefix('=')?.to_owned()))),
        }
    })?;
    let value = match (takes_value, inline.as_deref()) {
        (false, None) => None,
        (false, Some(_)) => return None,
        (true, Some(value)) => Some(value.to_owned()),
        (true, None) => Some(args.get(at + 1)?.to_str()?.to_owned()),
    };
    if only.is_some_and(|only| value.as_deref() != Some(only)) {
        return None;
    }
    let taken = if takes_value && inline.is_none() {
        2
    } else {
        1
    };
    args.drain(at..at + taken);
    Some(json!({"option":option,"reason":reason}))
}

fn unexpected(error: &clap::Error) -> Option<String> {
    if error.kind() != ErrorKind::UnknownArgument {
        return None;
    }
    match error.get(ContextKind::InvalidArg)? {
        ContextValue::String(argument) => Some(argument.clone()),
        _ => None,
    }
}

/// The note on a page size given to a response that has no pages.
fn paging(cli: &mut Cli) -> Result<Option<Value>, clap::Error> {
    if (cli.output.limit.is_none() && cli.output.offset.is_none()) || paged(&cli.command) {
        return Ok(None);
    }
    // A later page of a response that has none would be its first again.
    if cli.output.offset.is_some() {
        return Err(clap::Error::raw(
            ErrorKind::ArgumentConflict,
            "--offset requires workflow read/status/history, spec check, schema without --pointer, check, status, search, search-formats, operations list, diff without --out, or export without --out",
        ));
    }
    cli.output.limit = None;
    Ok(Some(
        json!({"option":"--limit","reason":"this command's response is not paged"}),
    ))
}

fn paged(command: &Command) -> bool {
    matches!(
        command,
        Command::Spec {
            command: SpecCommand::Check { out: None, .. }
        } | Command::Spec {
            command: SpecCommand::Workflow {
                command: WorkflowCommand::Read { .. }
                    | WorkflowCommand::Status { .. }
                    | WorkflowCommand::History,
                ..
            }
        } | Command::Documents {
            command: DocumentCommand::Schema { pointer: None, .. }
                | DocumentCommand::Status { .. }
                | DocumentCommand::Check { .. }
                | DocumentCommand::Search { .. }
                | DocumentCommand::SearchFormats { .. }
                | DocumentCommand::Operations {
                    command: OperationCommand::List { .. }
                }
                | DocumentCommand::Diff { out: None, .. }
                | DocumentCommand::Export { out: None, .. },
            ..
        }
    )
}

/// The subcommands an argument list names, from the program itself down.
struct Chain {
    root: clap::Command,
    names: Vec<String>,
}

impl Chain {
    fn of(args: &[OsString]) -> Self {
        let mut root = Cli::command();
        root.build();
        let mut names = Vec::new();
        let mut tokens = args.iter().skip(1).map(|arg| arg.to_string_lossy());
        while let Some(token) = tokens.next() {
            let current = Self::find(&root, &names);
            if token == "--" {
                break;
            }
            if let Some(long) = token.strip_prefix("--") {
                if !long.contains('=')
                    && current
                        .get_arguments()
                        .any(|arg| arg.get_long() == Some(long) && arg.get_action().takes_values())
                {
                    tokens.next();
                }
            } else if let Some(sub) = current.find_subcommand(token.as_ref()) {
                names.push(sub.get_name().to_owned());
            }
        }
        Self { root, names }
    }

    fn find<'a>(root: &'a clap::Command, names: &[String]) -> &'a clap::Command {
        names
            .iter()
            .fold(root, |command, name| command.find_subcommand(name).unwrap())
    }

    fn path(&self) -> String {
        self.names.join(" ")
    }

    /// What `error` needs beside its message for a corrected command line:
    /// the command it reached, the options and subcommands that command
    /// takes, and where a misplaced option belongs.
    fn details(&self, error: &clap::Error) -> Value {
        let command = Self::find(&self.root, &self.names);
        let mut details = Map::new();
        details.insert("command".into(), json!(self.path()));
        let accepted: Vec<String> = command
            .get_arguments()
            .filter(|arg| {
                !arg.is_hide_set() && !matches!(arg.get_id().as_str(), "help" | "version")
            })
            .filter_map(|arg| {
                let long = arg.get_long()?;
                Some(if arg.get_action().takes_values() {
                    let value = arg
                        .get_value_names()
                        .and_then(|names| names.first())
                        .map_or_else(|| arg.get_id().as_str().to_uppercase(), ToString::to_string);
                    format!("--{long} <{value}>")
                } else {
                    format!("--{long}")
                })
            })
            .collect();
        details.insert("accepted".into(), json!(accepted));
        let subcommands: Vec<&str> = command
            .get_subcommands()
            .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
            .map(clap::Command::get_name)
            .collect();
        if !subcommands.is_empty() {
            details.insert("subcommands".into(), json!(subcommands));
        }
        if let Some(argument) = unexpected(error).filter(|argument| argument.len() <= 256) {
            let name = argument
                .split_once('=')
                .map_or(argument.as_str(), |(name, _)| name);
            if let Some(hint) = self.misplaced(name) {
                details.insert("hint".into(), json!(hint));
            }
            details.insert("argument".into(), json!(argument));
        }
        Value::Object(details)
    }

    /// Where an option of an enclosing command goes, when it was typed after
    /// a subcommand that does not take it.
    fn misplaced(&self, name: &str) -> Option<String> {
        let long = name.strip_prefix("--")?;
        (1..self.names.len()).rev().find_map(|depth| {
            let owner = Self::find(&self.root, &self.names[..depth]);
            owner
                .get_arguments()
                .any(|arg| arg.get_long() == Some(long) && !arg.is_global_set())
                .then(|| {
                    format!(
                        "{name} is an option of `{}`: put it right after `{}`, before `{}`",
                        self.names[..depth].join(" "),
                        self.names[depth - 1],
                        self.names[depth],
                    )
                })
        })
    }
}

/// The message and details of a rejected command line.
pub fn invalid(error: &clap::Error, args: &[OsString]) -> (String, Value) {
    // clap's "try --help" hint addresses a terminal user, not an agent.
    let message = error.to_string();
    let message = message
        .split_once("\n\nFor more information, try")
        .map_or(message.as_str(), |(head, _)| head)
        .to_owned();
    (message, Chain::of(args).details(error))
}

/// Count, in the project's `.arp/cache/argument-errors.json`, the options
/// agents give a command it does not take, by outcome, so that what may be
/// ignored can follow what is actually mistaken. Outside a project nothing
/// is written, and a command never fails for this.
fn record(args: &[OsString], outcome: &str, keys: impl IntoIterator<Item = String>) {
    let keys: Vec<String> = keys.into_iter().collect();
    if keys.is_empty() {
        return;
    }
    let _ = (|| -> anyhow::Result<()> {
        let start = match root_argument(args) {
            Some(root) => root,
            None => std::env::current_dir()?,
        };
        let root = arp4_cli::project::discover(&start)?;
        let path = arp4_cli::data::under(&root, ".arp/cache/argument-errors.json")?;
        let mut counts: Value = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        if !counts[outcome].is_object() {
            counts[outcome] = json!({});
        }
        for key in keys {
            let count = &mut counts[outcome][&key];
            *count = json!(count.as_u64().unwrap_or(0) + 1);
        }
        arp4_cli::data::replace_cache(&path, &serde_json::to_vec_pretty(&counts)?)
    })();
}

fn root_argument(args: &[OsString]) -> Option<std::path::PathBuf> {
    let at = args.iter().position(|arg| arg == "--root");
    match at {
        Some(at) => args.get(at + 1).map(Into::into),
        None => args
            .iter()
            .find_map(|arg| arg.to_str()?.strip_prefix("--root=").map(Into::into)),
    }
}
