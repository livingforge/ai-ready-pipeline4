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
    Doctor,
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Documents {
        #[arg(long, global = true)]
        root: Option<PathBuf>,
        /// Include verification hashes in output: the content hash and the original's
        /// SHA-256 in check and status, and those of edits, reviews and exports.
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
/// The document, sheet and record of a row or column edit.
#[derive(clap::Args)]
pub(crate) struct EditTarget {
    /// Adopted document ID.
    #[arg(required_unless_present = "proposal")]
    pub(crate) document: Option<String>,
    /// Edit the proposal of this document ID instead, before record.
    #[arg(long, conflicts_with = "document")]
    pub(crate) proposal: Option<String>,
    /// Worksheet name; for Word the part (document, header-N, ...), for PowerPoint
    /// the page (slide-N, notes-N, or the operation ID of an inserted slide).
    #[arg(long)]
    pub(crate) sheet: String,
    /// Operation ID (^[a-zA-Z0-9][a-zA-Z0-9_-]*$); generated when omitted. Give one so
    /// that repeating the command, e.g. after a timeout, is reported unchanged.
    #[arg(long)]
    pub(crate) id: Option<String>,
    /// Why the rows or columns change; recorded with the operation.
    #[arg(long)]
    pub(crate) reason: String,
    /// The content hash from check --include-hashes; the edit is refused when the
    /// document changed since.
    #[arg(long)]
    pub(crate) base: Option<String>,
    /// Validate and report the edit without writing.
    #[arg(long)]
    pub(crate) dry_run: bool,
}
/// Where inserted rows or columns go.
#[derive(clap::Args)]
#[group(required = true, multiple = false)]
pub(crate) struct InsertPlace {
    /// Insert after this row or column: a row number (15 or r15) or column letters
    /// (C) of the original sheet, the key <operation ID>-<n> of one an earlier
    /// operation inserted, or last (the last one holding a value).
    #[arg(long)]
    pub(crate) after: Option<String>,
    /// Insert before this row or column, given like --after.
    #[arg(long)]
    pub(crate) before: Option<String>,
}
#[derive(Subcommand)]
pub(crate) enum RowCommand {
    /// Insert rows and write their values under the keys <operation ID>-<n>.
    Insert {
        #[command(flatten)]
        target: EditTarget,
        #[command(flatten)]
        place: InsertPlace,
        /// Number of rows; defaults to the number of --values items, or 1.
        #[arg(long)]
        count: Option<u32>,
        /// Original row whose formatting the new rows take; defaults to the row
        /// above them, as in Excel. Excel rows `9:10` are taken in turn by each
        /// group of new rows, as a form laid out over two rows.
        #[arg(long)]
        style_from: Option<String>,
        /// Excel only: make the merges within the style rows (--style-from, by
        /// default the row above) again over each group of new rows, as Excel's
        /// Insert Copied Cells does.
        #[arg(long)]
        merges: bool,
        /// YAML or JSON file, or - for stdin: a list with one object per new row,
        /// keyed by column letter, e.g. [{B: "8", C: 2025/11/21}]. Values take the
        /// type of the column (the --style-from row, or the nearest value above).
        #[arg(long)]
        values: Option<PathBuf>,
    },
    /// Delete rows of the original sheet together with their content values.
    Delete {
        #[command(flatten)]
        target: EditTarget,
        /// First row to delete: a number of the original sheet (15 or r15).
        #[arg(long)]
        from: String,
        /// Number of rows to delete.
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
}
#[derive(Subcommand)]
pub(crate) enum ColumnCommand {
    /// Move a contiguous column block. Apply insertion/deletion operations first.
    Move {
        #[command(flatten)]
        target: EditTarget,
        #[command(flatten)]
        place: InsertPlace,
        #[arg(long)]
        from: String,
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
    /// Insert columns and write their values under the keys <operation ID>-<n>.
    Insert {
        #[command(flatten)]
        target: EditTarget,
        #[command(flatten)]
        place: InsertPlace,
        /// Number of columns; defaults to the number of --values items, or 1.
        #[arg(long)]
        count: Option<u32>,
        /// YAML or JSON file, or - for stdin: a list with one object per new column,
        /// keyed by row of the original sheet, e.g. [{r8: 備考, r9: "1"}].
        /// Values are stored as given.
        #[arg(long)]
        values: Option<PathBuf>,
    },
    /// Delete columns of the original sheet together with their content values.
    Delete {
        #[command(flatten)]
        target: EditTarget,
        /// First column to delete: letters of the original sheet (C).
        #[arg(long)]
        from: String,
        /// Number of columns to delete.
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
}
/// The document and record of a slide edit.
#[derive(clap::Args)]
pub(crate) struct SlideTarget {
    /// Adopted document ID of a PowerPoint presentation.
    #[arg(required_unless_present = "proposal")]
    pub(crate) document: Option<String>,
    /// Edit the proposal of this document ID instead, before record.
    #[arg(long, conflicts_with = "document")]
    pub(crate) proposal: Option<String>,
    /// Operation ID (^[a-zA-Z0-9][a-zA-Z0-9_-]*$); generated when omitted. An
    /// inserted slide is named by it. Give one so that repeating the command,
    /// e.g. after a timeout, is reported unchanged.
    #[arg(long)]
    pub(crate) id: Option<String>,
    /// Why the slide is added or removed; recorded with the operation.
    #[arg(long)]
    pub(crate) reason: String,
    /// The content hash from check --include-hashes; the edit is refused when the
    /// document changed since.
    #[arg(long)]
    pub(crate) base: Option<String>,
    /// Validate and report the edit without writing.
    #[arg(long)]
    pub(crate) dry_run: bool,
}
/// Where an inserted slide goes.
#[derive(clap::Args)]
#[group(required = true, multiple = false)]
pub(crate) struct SlidePlace {
    /// Insert after this slide: slide-N of the original, or the operation ID of
    /// a slide an earlier operation inserted.
    #[arg(long)]
    pub(crate) after: Option<String>,
    /// Insert before this slide, given like --after.
    #[arg(long)]
    pub(crate) before: Option<String>,
}
/// The document, slide and record of a shape edit.
#[derive(clap::Args)]
pub(crate) struct ShapeTarget {
    /// Adopted document ID of a PowerPoint presentation.
    #[arg(required_unless_present = "proposal")]
    pub(crate) document: Option<String>,
    /// Edit the proposal of this document ID instead, before record.
    #[arg(long, conflicts_with = "document")]
    pub(crate) proposal: Option<String>,
    /// Slide: slide-N of the original, or the operation ID of a slide a slide
    /// operation inserted.
    #[arg(long)]
    pub(crate) slide: String,
    /// Operation ID (^[a-zA-Z0-9][a-zA-Z0-9_-]*$); generated when omitted. Name
    /// an added shape by it in later shape edits.
    #[arg(long)]
    pub(crate) id: Option<String>,
    /// Why the shape changes; recorded with the operation.
    #[arg(long)]
    pub(crate) reason: String,
    /// The content hash from check --include-hashes; the edit is refused when the
    /// document changed since.
    #[arg(long)]
    pub(crate) base: Option<String>,
    /// Validate and report the edit without writing.
    #[arg(long)]
    pub(crate) dry_run: bool,
}
/// Where a shape goes on the slide, in points from its top left corner.
#[derive(clap::Args)]
pub(crate) struct ShapePlace {
    #[arg(long, allow_hyphen_values = true)]
    pub(crate) left: Option<f64>,
    #[arg(long, allow_hyphen_values = true)]
    pub(crate) top: Option<f64>,
    #[arg(long)]
    pub(crate) width: Option<f64>,
    #[arg(long)]
    pub(crate) height: Option<f64>,
    /// Rotation in degrees, clockwise.
    #[arg(long, allow_hyphen_values = true)]
    pub(crate) rotation: Option<f64>,
}
/// How a shape is drawn.
#[derive(clap::Args)]
pub(crate) struct ShapeLook {
    /// Fill color as RRGGBB, or none.
    #[arg(long)]
    pub(crate) fill: Option<String>,
    /// Line color as RRGGBB, or none.
    #[arg(long)]
    pub(crate) line: Option<String>,
    /// Line weight in points.
    #[arg(long)]
    pub(crate) line_weight: Option<f64>,
}
#[derive(Subcommand)]
pub(crate) enum ImageCommand {
    /// Add a PNG picture from the document's assets folder, its corners at the
    /// top left of two cells. Like a picture Excel inserts, it moves with its
    /// top left cell and keeps its size.
    Add {
        /// Adopted document ID of an Excel workbook.
        #[arg(required_unless_present = "proposal")]
        document: Option<String>,
        /// Edit the proposal of this document ID instead, before record.
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
        /// Worksheet of the original.
        #[arg(long)]
        sheet: String,
        /// Cell at the picture's top left corner, such as B2.
        #[arg(long)]
        from: String,
        /// Cell at the picture's bottom right corner, right of and below --from.
        #[arg(long)]
        to: String,
        /// The picture: assets/<file>.png in the document folder.
        #[arg(long)]
        asset: String,
        /// Name of the picture.
        #[arg(long)]
        name: Option<String>,
        /// Operation ID (^[a-zA-Z0-9][a-zA-Z0-9_-]*$); generated when omitted.
        #[arg(long)]
        id: Option<String>,
        /// Why the picture is added; recorded with the operation.
        #[arg(long)]
        reason: String,
        /// The content hash from check --include-hashes; the edit is refused when the
        /// document changed since.
        #[arg(long)]
        base: Option<String>,
        /// Validate and report the edit without writing.
        #[arg(long)]
        dry_run: bool,
    },
}
#[derive(Subcommand)]
pub(crate) enum ShapeCommand {
    /// Change the place, size, rotation, fill, line or name of a shape, picture,
    /// connector, group or table frame. Unset properties stay as they are.
    Update {
        #[command(flatten)]
        target: ShapeTarget,
        /// Shape to change: its drawing ID.
        #[arg(long)]
        shape: String,
        #[command(flatten)]
        place: ShapePlace,
        #[command(flatten)]
        look: ShapeLook,
        /// New name of the shape.
        #[arg(long)]
        name: Option<String>,
    },
    /// Add a rectangle, rounded rectangle, ellipse, text box or line, with
    /// PowerPoint's default shape style unless --fill and --line are given.
    Add {
        #[command(flatten)]
        target: ShapeTarget,
        #[arg(long = "type", value_parser = ["rectangle", "rounded_rectangle", "ellipse", "textbox", "line"])]
        shape_type: String,
        #[arg(long)]
        name: String,
        #[command(flatten)]
        place: ShapePlace,
        #[command(flatten)]
        look: ShapeLook,
        /// Text of the shape; a line break starts a paragraph. After apply it is
        /// edited in the slide's content page like other text.
        #[arg(long)]
        text: Option<String>,
    },
    /// Delete a shape, picture, connector or group without text. A shape
    /// showing the slide's text or joined by a connector is refused.
    Delete {
        #[command(flatten)]
        target: ShapeTarget,
        #[arg(long)]
        shape: String,
    },
    /// Add a PNG picture from the document's assets folder.
    AddPicture {
        #[command(flatten)]
        target: ShapeTarget,
        /// The picture: assets/<file>.png in the document folder.
        #[arg(long)]
        asset: String,
        #[arg(long)]
        name: String,
        /// Alternative text of the picture.
        #[arg(long)]
        description: Option<String>,
        #[command(flatten)]
        place: ShapePlace,
    },
    /// Show another PNG picture in a picture, keeping its place and size.
    ReplacePicture {
        #[command(flatten)]
        target: ShapeTarget,
        #[arg(long)]
        shape: String,
        #[arg(long)]
        asset: String,
    },
    /// Add a connector joining two shapes at connection sites: a rectangle's
    /// are 0 top, 1 left, 2 bottom and 3 right; an ellipse's 0 top to 7,
    /// counterclockwise every 45 degrees.
    AddConnector {
        #[command(flatten)]
        target: ShapeTarget,
        #[arg(long = "type", value_parser = ["straight", "elbow", "curve"])]
        connector_type: String,
        #[arg(long)]
        name: String,
        /// Shape the connector starts at.
        #[arg(long)]
        from: String,
        #[arg(long)]
        from_site: u32,
        /// Shape the connector ends at.
        #[arg(long)]
        to: String,
        #[arg(long)]
        to_site: u32,
        #[command(flatten)]
        look: ShapeLook,
    },
}
#[derive(Subcommand)]
pub(crate) enum SlideCommand {
    /// Insert a copy of a slide and its notes page, as PowerPoint's Duplicate
    /// Slide does, with their text in new content pages named by the operation ID
    /// (content/<ID>.yml, content/notes-<ID>.yml). Comments are not copied.
    Insert {
        #[command(flatten)]
        target: SlideTarget,
        /// Slide to copy: slide-N of the original, or the operation ID of a slide
        /// an earlier operation inserted. The copy takes its current content values.
        #[arg(long)]
        from: String,
        #[command(flatten)]
        place: SlidePlace,
    },
    /// Add a new slide made from a slide layout, as PowerPoint's New Slide
    /// does: one empty placeholder for each of the layout's placeholders (but
    /// date, footer and slide number), whose text is written in the new
    /// content page named by the operation ID (content/<ID>.yml).
    Add {
        #[command(flatten)]
        target: SlideTarget,
        /// Layout to make the slide from: its name or part, as the extraction's
        /// slide_layouts list them.
        #[arg(long)]
        layout: String,
        #[command(flatten)]
        place: SlidePlace,
    },
    /// Delete a slide of the original with its notes page and their content
    /// pages. Refused while another slide links to it.
    Delete {
        #[command(flatten)]
        target: SlideTarget,
        /// Slide to delete: slide-N of the original.
        #[arg(long)]
        slide: String,
    },
    /// Move a slide, with its notes page, next to another slide in the show
    /// order. In a presentation with sections it joins the other slide's
    /// section. Custom shows keep their own order.
    Move {
        #[command(flatten)]
        target: SlideTarget,
        /// Slide to move: slide-N of the original, or the operation ID of a slide
        /// an earlier operation inserted.
        #[arg(long)]
        slide: String,
        #[command(flatten)]
        place: SlidePlace,
    },
    /// Hide a slide from the slide show; it stays in the presentation.
    Hide {
        #[command(flatten)]
        target: SlideTarget,
        /// Slide to hide, given like move's --slide.
        #[arg(long)]
        slide: String,
    },
    /// Show a hidden slide in the slide show again.
    Show {
        #[command(flatten)]
        target: SlideTarget,
        /// Slide to show, given like move's --slide.
        #[arg(long)]
        slide: String,
    },
}
/// The documents record, adopt and review take: one document, every document
/// below a folder, or every document; those in another state are skipped.
#[derive(clap::Args)]
pub(crate) struct Targets {
    /// Document ID, or a folder ID for every document below it.
    #[arg(required_unless_present = "all")]
    pub document: Option<String>,
    /// Take every document that awaits this step.
    #[arg(long, conflicts_with = "document")]
    pub all: bool,
    /// List the documents this would take and those it skips, without writing.
    #[arg(long)]
    pub dry_run: bool,
    /// With --dry-run, save the plan to this new file for --expect.
    #[arg(long, requires = "dry_run")]
    pub out: Option<PathBuf>,
    /// A plan saved by --dry-run --out: take only the documents it lists, and only
    /// while their content is the one it lists.
    #[arg(long)]
    pub expect: Option<PathBuf>,
}
/// A contract `documents schema` prints, refused with the list of contracts.
fn schema_kind(kind: &str) -> Result<String, String> {
    let schemas = arp4_cli::schemas();
    if schemas.get(kind).is_some() {
        return Ok(kind.to_owned());
    }
    let kinds: Vec<&String> = schemas
        .as_object()
        .map(|s| s.keys().collect())
        .unwrap_or_default();
    Err(format!(
        "unknown contract {kind}; the contracts are {}",
        kinds
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}
#[derive(Subcommand)]
pub(crate) enum OperationCommand {
    /// List the pending row, column, slide, shape and image operations of an
    /// adopted document or proposal, in the order apply writes them.
    List {
        /// Adopted document ID.
        #[arg(required_unless_present = "proposal")]
        document: Option<String>,
        /// List the operations of the proposal of this document ID instead.
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
    },
    /// Take back a pending operation with what it did to the content: the
    /// values of rows, columns and slides it inserted leave the content, and
    /// those it deleted come back as the original holds them. Refused while
    /// another operation depends on it (a later row, column or image operation
    /// of the same sheet, or one naming what it added); take those back first.
    Remove {
        /// Adopted document ID.
        #[arg(required_unless_present = "proposal")]
        document: Option<String>,
        /// Edit the proposal of this document ID instead, before record.
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
        /// ID of the operation, as operations list gives it.
        #[arg(long)]
        id: String,
        /// The content hash from check --include-hashes; the removal is refused
        /// when the document changed since.
        #[arg(long)]
        base: Option<String>,
        /// Validate and report the removal without writing.
        #[arg(long)]
        dry_run: bool,
    },
}
#[derive(Subcommand)]
pub(crate) enum DocumentCommand {
    /// Print a JSON contract by name, as a list of its properties; --pointer
    /// selects one node and --full prints all of it. An unknown name lists the
    /// contracts.
    Schema {
        /// The contract, e.g. value-edits, document-elements or mappings.
        #[arg(value_parser = schema_kind)]
        kind: String,
        /// Select a schema node using a JSON Pointer, e.g. /properties/entries/items.
        #[arg(long)]
        pointer: Option<String>,
    },
    /// Make the project's .arp folder and configuration.
    Init {
        /// Folder holding the originals, relative to the project root.
        #[arg(long, default_value = "docs")]
        sources: String,
    },
    /// Import an original, or every original below a folder, as proposals. The document
    /// ID is the original's path below the sources folder. Unchanged originals are skipped.
    /// An encrypted Office original is decrypted and saved over without its encryption
    /// first: a password with --password-stdin, IRM and sensitivity labels by desktop
    /// Office on Windows with the signed-in account's rights.
    Import {
        source: PathBuf,
        /// Re-extract even when the original's SHA-256 matches a proposal or adopted document.
        #[arg(long)]
        force: bool,
        /// Read passwords of encrypted originals from standard input, one per line;
        /// each encrypted original is opened with the first that fits.
        #[arg(long)]
        password_stdin: bool,
        /// Adopt each new proposal as extracted, without record and review: the
        /// document is `imported` until edited. A proposal whose adopted document
        /// has edits or a review is kept for record and adopt (`not_adopted`).
        #[arg(long)]
        adopt: bool,
    },
    /// Remove the documents at or below an ID whose originals were moved, renamed or deleted.
    Remove { document: String },
    /// Discard one proposal, leaving the original and adopted document untouched.
    Discard { document: String },
    /// Search adopted extraction passages with Japanese segmentation, BM25 and
    /// source locations from the last explicitly refreshed local index.
    Search {
        /// One to 32 quoted queries against one saved index snapshot. Within each query,
        /// whitespace separates literal AND terms; punctuation is not query syntax.
        #[arg(required = true, num_args = 1..=32)]
        query: Vec<String>,
        /// Restrict to a document ID or folder ID.
        #[arg(long)]
        document: Option<String>,
        /// Project vocabulary groups (search-synonyms schema). Defaults to .arp/search-synonyms.yml if present.
        #[arg(long)]
        synonyms: Option<PathBuf>,
        /// Emit disjoint search wall-clock timings as one JSON record on stderr.
        #[arg(long)]
        profile: bool,
        /// Pin pagination to a previous index and query revision.
        #[arg(long)]
        revision: Option<String>,
    },
    /// Reconcile canonical documents with the local search index.
    SearchRefresh {
        /// Recreate the disposable index before refreshing all documents.
        #[arg(long)]
        rebuild: bool,
    },
    /// Write a derived structure view from the document's canonical corrections,
    /// with the report of how they replay on its extraction.
    StructureRead {
        document: String,
        #[arg(long)]
        out: PathBuf,
        /// Read the proposal instead of the adopted document.
        #[arg(long)]
        proposal: bool,
    },
    /// Write the element metadata of every page, with current fonts resolved, and
    /// the visuals its references point to (the document-elements contract): the
    /// IDs, table rows and columns, headers, spans and formula kinds of the body.
    ElementsRead {
        document: String,
        #[arg(long)]
        out: PathBuf,
        /// Read the proposal instead of the adopted document.
        #[arg(long)]
        proposal: bool,
    },
    /// Write the extraction with every cell's font, style and cached result in
    /// place (the extraction-view contract): extraction.json stores fonts and
    /// styles once per sheet and leaves out the cached results.
    ExtractionRead {
        document: String,
        #[arg(long)]
        out: PathBuf,
        /// Read the proposal instead of the adopted document.
        #[arg(long)]
        proposal: bool,
    },
    /// Save structure corrections; remove a managed work/structure view on success.
    StructureSave {
        document: String,
        #[arg(long)]
        input: PathBuf,
        /// Save into the proposal, before it is recorded and adopted.
        #[arg(long)]
        proposal: bool,
    },
    /// Insert or delete Excel worksheet rows, or Word and PowerPoint paragraphs and
    /// table rows. The operation in mappings.yml and the content values it adds or
    /// removes are written together, after the same validation as check.
    Rows {
        #[command(subcommand)]
        command: RowCommand,
    },
    /// Insert or delete Excel worksheet columns, like rows.
    Columns {
        #[command(subcommand)]
        command: ColumnCommand,
    },
    /// Copy, add, delete, move, hide or show PowerPoint slides. The operation in
    /// mappings.yml and the content pages it adds or removes are written
    /// together, after the same validation as check.
    Slides {
        #[command(subcommand)]
        command: SlideCommand,
    },
    /// Change, add or delete shapes, pictures and connectors of a PowerPoint
    /// slide. Places and sizes are points on the slide, as the extraction's
    /// drawings give them; a shape is named by its drawing ID
    /// (<slide part>#<id>) or by the operation ID of the operation adding it.
    Shapes {
        #[command(subcommand)]
        command: ShapeCommand,
    },
    /// Add PNG pictures to an Excel worksheet over a cell range. The operation in
    /// mappings.yml is written after a trial write and the same validation as check.
    Images {
        #[command(subcommand)]
        command: ImageCommand,
    },
    /// Record who formed a proposal, with which model and prompt. A folder ID or
    /// --all records every proposal below it that needs a record.
    Record {
        #[command(flatten)]
        targets: Targets,
        #[arg(long)]
        model: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        prompt: PathBuf,
    },
    /// Adopt a recorded proposal as the document without marking it reviewed.
    /// A folder ID or --all adopts every proposal below it that is ready.
    Adopt {
        #[command(flatten)]
        targets: Targets,
    },
    /// Review the edits of an adopted document. A folder ID or --all reviews every
    /// document below it that needs a review.
    Review {
        #[command(flatten)]
        targets: Targets,
        #[arg(long)]
        reviewer: String,
    },
    /// Show what changed: a proposal against its adopted document, or with
    /// --document an adopted document against the version Git's HEAD holds.
    Diff {
        /// Document ID whose proposal is compared.
        #[arg(conflicts_with = "document")]
        proposal: Option<String>,
        /// Compare this adopted document with Git's HEAD instead.
        #[arg(long)]
        document: Option<String>,
        /// Format of the --out file; standard output is JSON.
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
        /// Save the complete differences to this new file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Write the exact export candidate whose layout was confirmed to the
    /// original, then re-extract it as a proposal formed by apply, ready to adopt.
    Apply { document: String },
    /// Record a human layout review of an exported candidate and its exact hash.
    ConfirmExport {
        document: String,
        /// The exported file that was opened and checked.
        #[arg(long)]
        candidate: PathBuf,
        /// output_sha256 of the candidate's export report.
        #[arg(long)]
        output_sha256: String,
        /// Who looked at the candidate.
        #[arg(long)]
        reviewer: String,
        /// What was looked at, and what was found.
        #[arg(long)]
        reason: String,
        /// Open the candidate and check changed cells and surrounding layout first.
        #[arg(long)]
        layout_reviewed: bool,
    },
    /// Edit values, formulas, table labels, shape texts and fonts of Office and
    /// PDF content, and give values to empty Excel cells, as the value-edits
    /// contract describes (documents schema value-edits). Targets are cells of
    /// the extraction or element IDs of elements-read, so the values of rows
    /// and slides operations insert are edited too. Every edit is checked, and
    /// written by the original's writer to a scratch file, before any is saved;
    /// the request is recorded with the content.
    Values {
        /// Adopted document ID.
        #[arg(required_unless_present = "proposal")]
        document: Option<String>,
        /// Edit the proposal of this document ID instead, before record.
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
        /// The value-edits request, a JSON or YAML file, or - for stdin. Its
        /// base and source_sha256 come from check --include-hashes.
        #[arg(long)]
        input: PathBuf,
        /// Validate and report the edits without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Without --out, plan the export of a reviewed document: what it writes
    /// to its original and what it leaves out. With --out, write the original
    /// with the edits to a new file and its report beside it.
    Export {
        document: String,
        /// New file inside .arp/cache/export, with the original's extension.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Render a page of an exported candidate to a PNG image, to look at it
    /// before confirm-export: an Excel sheet or range, a Word or PDF page, or
    /// a PowerPoint slide. Needs Windows and desktop Office (Excel; Word or
    /// PowerPoint for their documents); PDF pages are rendered by Windows.
    Render {
        document: String,
        /// The exported file, beside its .report.json.
        #[arg(long)]
        candidate: PathBuf,
        /// New PNG file to write.
        #[arg(long)]
        out: PathBuf,
        /// Excel worksheet to render.
        #[arg(long)]
        sheet: Option<String>,
        /// Excel A1 range; the sheet's used range when omitted.
        #[arg(long)]
        range: Option<String>,
        /// Page of a Word or PDF document, or slide of a presentation in the
        /// candidate's order, from 1.
        #[arg(long, default_value_t = 1)]
        page: u32,
        /// Seconds to wait for Office.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
    },
    /// List or take back pending row, column, slide, shape and image operations.
    Operations {
        #[command(subcommand)]
        command: OperationCommand,
    },
    /// Check documents and proposals as edits leave them: their state, and the
    /// blockers or error keeping each from the next step. --include-hashes adds
    /// the content hash and the original's SHA-256 that value edits are based on.
    Check {
        document: Option<String>,
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
        /// Fail unless the documents are reviewed.
        #[arg(long)]
        require_reviewed: bool,
    },
    /// The state of documents and proposals, like check.
    Status {
        document: Option<String>,
        #[arg(long, conflicts_with = "document")]
        proposal: Option<String>,
    },
}
