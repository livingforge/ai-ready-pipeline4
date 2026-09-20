use anyhow::{Context, Result, bail, ensure};
use arp4_cli::{data::*, documents::Store};
use clap::{Parser, Subcommand, ValueEnum};
use std::{fs, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "ARP Rust: Excel document workflow (XML cell writeback)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Doctor {
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Documents {
        #[arg(long, global = true)]
        root: Option<PathBuf>,
        #[command(subcommand)]
        command: DocumentCommand,
    },
}
#[derive(Clone, ValueEnum)]
enum Format {
    Text,
    Json,
    Markdown,
}
#[derive(Subcommand)]
enum SkillCommand {
    Install {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, value_enum, default_value = "all")]
        agent: arp4_cli::skills::Agent,
    },
}
#[derive(Subcommand)]
enum DocumentCommand {
    Schema {
        kind: String,
    },
    Init {
        #[arg(long, default_value = "knowledge")]
        directory: String,
    },
    Import {
        source: PathBuf,
        #[arg(long)]
        id: String,
    },
    Record {
        proposal: String,
        #[arg(long)]
        model: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        prompt: PathBuf,
    },
    Adopt {
        proposal: String,
        #[arg(long)]
        reviewer: String,
    },
    Review {
        document: String,
        #[arg(long)]
        reviewer: String,
    },
    Diff {
        #[arg(conflicts_with = "document")]
        proposal: Option<String>,
        #[arg(long)]
        document: Option<String>,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Export {
        document: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "auto")]
        engine: String,
    },
    Check {
        document: Option<String>,
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
        #[arg(long)]
        require_reviewed: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    Status {
        document: Option<String>,
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
}
struct Lock {
    path: PathBuf,
    file: Option<fs::File>,
}
impl Drop for Lock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}
fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Doctor { format } => {
            let report = arp4_cli::capabilities();
            if matches!(format, Format::Json) {
                println!("{}", serde_json::to_string_pretty(&report)?)
            } else {
                println!(
                    "ARP {} Rust\nAvailable: skills install; documents init/import/check/status/record/diff/adopt/review/export/schema.\nExcel: .xlsx/.xlsm scalar cells via XML; Python is not required.\nNot implemented: structural/formula writeback, OCR, non-cell extraction, other document formats, spec, edit-plan.\nEmpty-Windows and Excel COM acceptance remain unverified; not release-ready.",
                    env!("CARGO_PKG_VERSION")
                );
            }
        }
        Command::Skills {
            command: SkillCommand::Install { root, agent },
        } => println!(
            "Installed {} skill files in {}",
            arp4_cli::skills::install(&root, agent)?,
            root.display()
        ),
        Command::Documents { root, command } => {
            if let DocumentCommand::Schema { kind } = &command {
                let schemas = arp4_cli::schemas();
                let schema = schemas.get(kind).context("unknown document schema")?;
                println!("{}", serde_json::to_string_pretty(schema)?);
                return Ok(());
            }
            let root = if let Some(root) = root {
                root
            } else {
                let mut root = std::env::current_dir()?;
                if !matches!(command, DocumentCommand::Init { .. }) {
                    while !root.join(".arp/config.yml").exists() {
                        ensure!(root.pop(), "no .arp/config.yml found; run documents init");
                    }
                }
                root
            };
            if matches!(command, DocumentCommand::Init { .. }) {
                fs::create_dir_all(&root)?;
            }
            let root = dunce::canonicalize(&root)?;
            let mutate = matches!(
                command,
                DocumentCommand::Init { .. }
                    | DocumentCommand::Import { .. }
                    | DocumentCommand::Record { .. }
                    | DocumentCommand::Adopt { .. }
                    | DocumentCommand::Review { .. }
                    | DocumentCommand::Export { out: Some(_), .. }
            );
            let _lock = if mutate {
                let path = under(&root, ".arp/rust-documents.lock")?;
                fs::create_dir_all(path.parent().unwrap())?;
                let file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .context(
                        "another Rust document operation is running (or stale lock remains)",
                    )?;
                Some(Lock {
                    path,
                    file: Some(file),
                })
            } else {
                None
            };
            if let DocumentCommand::Init { directory } = command {
                Store::init(&root, &directory)?;
                println!("Initialized {}", root.display());
                return Ok(());
            }
            let store = Store::open(&root)?;
            let result = match command {
                DocumentCommand::Import { source, id } => store.import(&source, &id)?,
                DocumentCommand::Record {
                    proposal,
                    model,
                    actor,
                    prompt,
                } => store.record(&proposal, &model, &actor, &prompt)?,
                DocumentCommand::Adopt { proposal, reviewer } => {
                    store.adopt(&proposal, &reviewer)?
                }
                DocumentCommand::Review { document, reviewer } => {
                    store.review(&document, &reviewer)?
                }
                DocumentCommand::Export {
                    document,
                    out,
                    engine,
                } => store.export(&document, out.as_deref(), &engine)?,
                DocumentCommand::Diff {
                    proposal,
                    document,
                    format,
                    out,
                } => {
                    let result = store.diff(proposal.as_deref(), document.as_deref())?;
                    let rendered = if matches!(format, Format::Json) {
                        serde_json::to_string_pretty(&result)?
                    } else {
                        let mut text = String::from("# Document differences\n");
                        for comparison in array(&result["comparisons"])? {
                            text.push_str(&format!("\n## {}\n", string(&comparison["kind"])?));
                            for change in array(&comparison["changes"])? {
                                text.push_str(&format!(
                                    "\n- {} ({})\n  before: {}\n  after: {}\n",
                                    string(&change["path"])?,
                                    string(&change["kind"])?,
                                    change["before"],
                                    change["after"]
                                ));
                            }
                        }
                        text
                    };
                    if let Some(path) = out {
                        let absolute = if path.is_absolute() {
                            path
                        } else {
                            std::env::current_dir()?.join(path)
                        };
                        let parent = absolute.parent().context("missing output parent")?;
                        let mut ancestor = parent;
                        while !ancestor.exists() {
                            ancestor = ancestor
                                .parent()
                                .context("missing existing output ancestor")?;
                        }
                        let suffix = parent.strip_prefix(ancestor)?;
                        ensure!(
                            suffix
                                .components()
                                .all(|c| matches!(c, std::path::Component::Normal(_))),
                            "invalid output path"
                        );
                        let parent = dunce::canonicalize(ancestor)?.join(suffix);
                        ensure!(
                            !parent.starts_with(&store.directory)
                                && (!parent.starts_with(&store.arp)
                                    || parent.starts_with(store.arp.join("out"))),
                            "report must be outside document data or in .arp/out"
                        );
                        fs::create_dir_all(&parent)?;
                        let path =
                            parent.join(absolute.file_name().context("missing output filename")?);
                        ensure!(!path.exists(), "report already exists");
                        immutable(&path, rendered.as_bytes())?;
                        println!("{}", path.display())
                    } else {
                        println!("{rendered}")
                    };
                    return Ok(());
                }
                DocumentCommand::Check {
                    document,
                    proposal,
                    require_reviewed,
                    ..
                } => {
                    let result =
                        store.status(document.as_deref(), proposal.as_deref(), require_reviewed)?;
                    println!("{}", serde_json::to_string_pretty(&result)?);
                    if array(&result)?.iter().any(|r| r["state"] == "invalid") {
                        bail!("document validation failed")
                    }
                    return Ok(());
                }
                DocumentCommand::Status {
                    document, proposal, ..
                } => {
                    let result = store.status(document.as_deref(), proposal.as_deref(), false)?;
                    println!("{}", serde_json::to_string_pretty(&result)?);
                    if array(&result)?.iter().any(|r| r["state"] == "invalid") {
                        bail!("invalid document state")
                    }
                    return Ok(());
                }
                _ => unreachable!(),
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run(Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("arp4: {e:#}");
            std::process::ExitCode::from(2)
        }
    }
}
