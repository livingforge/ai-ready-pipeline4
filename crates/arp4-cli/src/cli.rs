use crate::response::Output;
use crate::workspace;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "ARP Rust: Excel, Word, PPTX and PDF document workflow"
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) output: Output,
    #[command(subcommand)]
    pub(crate) command: Command,
}
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Build and validate evidence-backed specification models.
    Spec {
        #[command(subcommand)]
        command: SpecCommand,
    },
    Doctor {
        #[arg(long, value_enum, default_value = "json")]
        format: JsonFormat,
    },
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Documents {
        #[arg(long, global = true)]
        root: Option<PathBuf>,
        /// Include verification hashes in record/review/check/status/export output.
        #[arg(long, global = true)]
        include_hashes: bool,
        #[command(subcommand)]
        command: DocumentCommand,
    },
}
#[derive(Subcommand)]
pub(crate) enum SpecCommand {
    /// Interpret original Excel structure without changing workbook contents or formatting.
    Structure {
        /// Repository root used to verify original and image paths.
        #[arg(long, global = true, default_value = ".")]
        root: PathBuf,
        #[command(subcommand)]
        command: arp4_cli::document_structure::StructureCommand,
    },
    /// Resume evidence-backed generation with pluggable model runners.
    Workflow {
        #[command(flatten)]
        paths: workspace::Options,
        #[command(subcommand)]
        command: arp4_cli::workflow::WorkflowCommand,
    },
    /// Prepare small semantic tasks and expand source-bound replies deterministically.
    Semantic {
        /// Project configuration for agent reading material; otherwise use the current repository.
        #[arg(long, global = true)]
        root: Option<PathBuf>,
        #[command(subcommand)]
        command: SemanticCommand,
    },
    /// Maintain canonical requirement/specification revisions and derived views.
    Registry {
        /// Repository root; otherwise discover .arp/config.yml.
        #[arg(long, global = true)]
        root: Option<PathBuf>,
        #[command(subcommand)]
        command: RegistryCommand,
    },
    /// Assign persistent sequential IDs using an explicit identity plan.
    AssignIds {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        /// Previous assigned model; its .ids.json ledger is required.
        #[arg(long)]
        previous: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Write complete source text in document/sheet/cell order.
    Sources {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        document: Option<String>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Save the full JSON Schema for extraction output.
    Schema {
        #[arg(long)]
        out: PathBuf,
    },
    /// Capture parser extraction JSON files as a fixed input bundle.
    Capture {
        #[arg(long, required = true, num_args = 1..)]
        extraction: Vec<PathBuf>,
        /// Reviewed documents whose canonical structure corrections should be captured.
        #[arg(long, num_args = 1.., requires = "root")]
        document: Vec<String>,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Write the extraction and independent review instructions.
    Prompt {
        #[arg(long)]
        out: PathBuf,
    },
    Check {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        model: PathBuf,
        /// Save all diagnostics to a file instead of a page on stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Render {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        draft: bool,
    },
}
#[derive(Subcommand)]
pub(crate) enum SemanticCommand {
    /// Plan document-level re-extraction; changed claims always require global review.
    Changes {
        #[arg(long)]
        before: PathBuf,
        #[arg(long)]
        after: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    Packet {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        document: String,
        #[arg(long)]
        out: PathBuf,
    },
    Assemble {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, required=true, num_args=1..)]
        reply: Vec<PathBuf>,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        out: PathBuf,
    },
    ReviewPacket {
        /// Explicit audit sources; common headings/notes belong in --context-source.
        #[arg(long, num_args=1.., requires="document", conflicts_with="sheet")]
        source: Vec<String>,
        #[arg(long, num_args=1.., requires="source")]
        context_source: Vec<String>,
        /// Review all source text on one sheet, retaining document source aliases.
        #[arg(long, requires = "document")]
        sheet: Option<String>,
        /// Emit expanded logical data for inspection/measurement instead of lossless tables.
        #[arg(long)]
        expanded: bool,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        document: Option<String>,
        #[arg(long)]
        out: PathBuf,
    },
    ReviewApply {
        /// Explicit source IDs omitted from independent review, with reasons.
        #[arg(long)]
        review_plan: Option<PathBuf>,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long, required=true, num_args=1..)]
        review: Vec<PathBuf>,
        #[arg(long)]
        reviewer: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Assign initial persistent IDs and remap the classification catalog. Not an update command.
    Finalize {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Also save the validated initial canonical registry revision.
        #[arg(long = "registry", requires = "project")]
        registry_out: bool,
        /// Project name for the initial canonical registry.
        #[arg(long, requires = "registry_out")]
        project: Option<String>,
    },
}
#[derive(Subcommand)]
pub(crate) enum RegistryCommand {
    /// Import reviewed extraction with explicit classifications; never auto-approve.
    Init {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        project: String,
    },
    /// Check a finalized bundle without creating a registry.
    Preflight {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        project: String,
    },
    /// Apply an explicit change against a fingerprinted base to the current registry.
    Apply {
        #[arg(long)]
        change: PathBuf,
        #[arg(long)]
        input: Vec<PathBuf>,
    },
    /// Verify integrity, evidence, lifecycle and relationships.
    Check,
    /// Render the current registry into .arp/cache/registry.
    Render,
}
#[derive(Clone, ValueEnum)]
pub(crate) enum Format {
    Json,
    Markdown,
}
#[derive(Clone, ValueEnum)]
pub(crate) enum JsonFormat {
    Json,
}
#[derive(Subcommand)]
pub(crate) enum SkillCommand {
    /// Install ARP skills, references and custom agents for the selected host.
    Install {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, value_enum, default_value = "all")]
        agent: arp4_cli::skills::Agent,
    },
}
#[derive(Subcommand)]
pub(crate) enum DocumentCommand {
    Schema {
        kind: String,
        /// Select a schema node using a JSON Pointer, e.g. /properties/entries/items.
        #[arg(long)]
        pointer: Option<String>,
    },
    Init {
        #[arg(long, default_value = "docs")]
        sources: String,
    },
    Import {
        source: PathBuf,
        /// ASCII ID: letters, digits, underscore or hyphen; start with a letter/digit.
        #[arg(long)]
        id: String,
    },
    /// Write a derived structure view from the document's canonical corrections.
    StructureRead {
        document: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Save structure corrections; remove a managed work/structure view on success.
    StructureSave {
        document: String,
        #[arg(long)]
        input: PathBuf,
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
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Apply reviewed edits to the original and re-extract for verification.
    Apply { document: String },
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
        #[arg(long, value_enum, default_value = "json")]
        format: JsonFormat,
    },
    Status {
        document: Option<String>,
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
        #[arg(long, value_enum, default_value = "json")]
        format: JsonFormat,
    },
}
