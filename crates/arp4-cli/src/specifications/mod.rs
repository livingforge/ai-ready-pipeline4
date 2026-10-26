//! Evidence-preserving specification extraction and deterministic validation.
pub(crate) use render::render_table;
mod capture;
pub use capture::{capture, load_input, load_model, validate_structure_requirements};
mod validation;
use validation::normalized;
pub(crate) use validation::warning_document;
pub use validation::{Indexed, InputIndex, assess};
mod render;
pub use render::{render, source_view};

use crate::data::{array, encoded, hash, read, string};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
#[path = "../specification_quantities.rs"]
mod quantities;
use quantities::quantity_diagnostic;
pub use quantities::{QuantityInterpretation, QuantityNormalization, QuantitySemantics};

pub const PROMPT: &str = include_str!("../../../../surface/prompts/specifications.md");
pub fn schema() -> Value {
    serde_json::from_str(include_str!(
        "../../../../contracts/specification-schema.json"
    ))
    .expect("embedded specification schema")
}

pub(crate) fn table_category_allowed(category: &impl Serialize) -> bool {
    static CATEGORIES: std::sync::LazyLock<Value> =
        std::sync::LazyLock::new(|| schema()["$defs"]["table_category"]["enum"].clone());
    CATEGORIES
        .as_array()
        .expect("table category enum")
        .contains(&serde_json::to_value(category).expect("serializable category"))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub document: String,
    pub location: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<CellContext>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellContext {
    pub sheet: String,
    pub cell: String,
    pub row: u32,
    pub column: u32,
    pub number_format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<crate::native_text::TextPosition>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub schema_version: u32,
    pub revisions: BTreeMap<String, String>,
    pub sources: Vec<Source>,
    pub warnings: Vec<String>,
    pub structure_requirements: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub structures: BTreeMap<String, Value>,
    /// The merged ranges of each sheet that has any, by document and sheet:
    /// recorded once for the sheet rather than with each of its cells.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub merges: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl Source {
    /// Where the source comes in its document's extraction packet.
    pub(crate) fn packet_order(&self) -> (&String, Option<(&String, u32, u32)>, &String) {
        (
            &self.document,
            self.context.as_ref().map(|c| (&c.sheet, c.row, c.column)),
            &self.location,
        )
    }
}

impl Input {
    /// The merged ranges of `sheet` of `document`; none for a sheet without.
    pub fn sheet_merges(&self, document: &str, sheet: &str) -> &[String] {
        self.merges
            .get(document)
            .and_then(|sheets| sheets.get(sheet))
            .map_or(&[], Vec::as_slice)
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub source: String,
    pub start: usize,
    pub end: usize,
    pub quote: String,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Requirement,
    Specification,
    Observation,
    Estimate,
    Reference,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    Composed {
        text: String,
        evidence: Vec<Span>,
        operator: ConditionOperator,
        reason: String,
    },
    Stated {
        text: String,
        evidence: Vec<Span>,
    },
    Unspecified,
    Assumed {
        text: String,
        reason: String,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOperator {
    All,
    Any,
}
/// The point a claim makes: claims with the same key state the same
/// subject, property and condition, so their values must agree. Spec checks
/// and registry approvals compare claims by this one key.
pub(crate) fn claim_key(
    subject: &str,
    property: &str,
    condition: &Condition,
) -> (String, String, String) {
    (
        validation::normalized(subject),
        validation::normalized(property),
        condition.comparison_key(),
    )
}

impl Condition {
    pub(crate) fn evidence(&self) -> Option<&[Span]> {
        match self {
            Self::Stated { evidence, .. } | Self::Composed { evidence, .. } => Some(evidence),
            _ => None,
        }
    }
    fn comparison_key(&self) -> String {
        match self {
            Self::Composed {
                evidence, operator, ..
            } => json!([
                "composed",
                operator,
                evidence
                    .iter()
                    .map(|e| normalized(&e.quote))
                    .collect::<Vec<_>>()
            ])
            .to_string(),
            _ => normalized(self.text())
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect(),
        }
    }
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Stated { text, .. }
            | Self::Composed { text, .. }
            | Self::Assumed { text, .. } => text,
            Self::Unspecified => "原文に明記なし",
        }
    }
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Eq,
    Lt,
    Lte,
    Gt,
    Gte,
    Approx,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AtomicValue {
    Quantity {
        amount: serde_json::Number,
        unit: String,
        comparison: Comparison,
        interpretation: QuantityInterpretation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<Span>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_basis: Option<Span>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        semantics_basis: Option<Box<Span>>,
        #[serde(default, skip_serializing_if = "QuantitySemantics::is_scalar")]
        semantics: QuantitySemantics,
    },
    /// A source-bound quantity whose interpretation is intentionally pending.
    /// This preserves the claim without pretending that amount/unit semantics
    /// were established or hiding it in a text value.
    QuantityExpression {
        basis: Span,
    },
    Text {
        text: String,
    },
    /// Original cells, not interpreted or normalized numeric claims.
    Table {
        title: Vec<Span>,
        description: Vec<Span>,
        cells: Vec<Span>,
        notes: Vec<Span>,
    },
}
impl AtomicValue {
    pub(crate) fn display(&self) -> String {
        match self {
            Self::Quantity {
                amount,
                unit,
                comparison,
                semantics,
                ..
            } => format!(
                "{} {}{}{}",
                match comparison {
                    Comparison::Eq => "=",
                    Comparison::Lt => "<",
                    Comparison::Lte => "≤",
                    Comparison::Gt => ">",
                    Comparison::Gte => "≥",
                    Comparison::Approx => "約",
                },
                amount,
                unit,
                match semantics {
                    QuantitySemantics::Scalar => "",
                    QuantitySemantics::Mean => "（平均）",
                    QuantitySemantics::Period => "（周期）",
                }
            ),
            Self::QuantityExpression { basis } => format!("数量解釈待ち: {}", basis.quote),
            Self::Text { text } => text.clone(),
            Self::Table { cells, .. } => format!("原表（{}セル、数値の再解釈なし）", cells.len()),
        }
    }
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Quantity {
                amount,
                unit,
                comparison,
                semantics,
                ..
            } => {
                let number = if amount.is_f64() {
                    amount.as_f64().unwrap().to_string()
                } else {
                    amount.to_string()
                };
                let (number, unit) = quantities::canonical_unit(&number, unit);
                json!(["quantity", number, unit, comparison, semantics]).to_string()
            }
            Self::QuantityExpression { basis } => {
                json!(["quantity_expression", basis.quote]).to_string()
            }
            Self::Text { text } => json!([
                "text",
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            ])
            .to_string(),
            Self::Table {
                title,
                description,
                cells,
                notes,
            } => json!(["table", title, description, cells, notes]).to_string(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: Kind,
    pub subject: String,
    pub property: String,
    pub condition: Condition,
    pub value: AtomicValue,
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    pub requirements: Vec<String>,
    pub evidence: Vec<Span>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    pub evidence: Span,
    pub reason: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    pub source: String,
    pub reviewer: String,
    pub rationale: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub candidates: Vec<String>,
    pub selected: String,
    pub reviewer: String,
    pub rationale: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub schema_version: u32,
    pub input_hash: String,
    pub items: Vec<Item>,
    pub exclusions: Vec<Exclusion>,
    pub audits: Vec<Audit>,
    pub decisions: Vec<Decision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewRecord>,
}
/// Explicit omissions are review policy, never source-text exclusions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewPlan {
    pub omitted_sources: BTreeMap<String, String>,
}
impl ReviewPlan {
    pub fn validate(&self, input: &Input) -> Result<()> {
        let known: BTreeSet<&str> = if self.omitted_sources.is_empty() {
            BTreeSet::new()
        } else {
            input.sources.iter().map(|s| s.id.as_str()).collect()
        };
        for (source, reason) in &self.omitted_sources {
            ensure!(
                known.contains(source.as_str()),
                "unknown review omission source"
            );
            validation::nonempty(reason)?;
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRecord {
    pub content_hash: String,
    pub plan: ReviewPlan,
    pub global: bool,
    pub findings: Vec<Value>,
    pub replies: Vec<Value>,
}
pub fn review_content_hash(model: &Model) -> String {
    let mut value = serde_json::to_value(model).expect("serializable model");
    value.as_object_mut().unwrap().remove("review");
    value.as_object_mut().unwrap().remove("audits");
    hash(&encoded(&value))
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub ready: bool,
    pub assurance: Value,
    pub summary: BTreeMap<String, usize>,
    pub coverage: Value,
    pub issues: Vec<Value>,
    /// Numbers in text values that fixed lexical rules excluded from quantity checks.
    pub quantity_exemptions: Vec<Value>,
}

/// The assessment of the files `input` and `model` that a page of `spec
/// check` shows: whether it is ready, its assurance, summary, coverage and
/// issues. Every page otherwise assesses the whole model again, so reading
/// all pages took time in proportion to the pages times the model. Inside a
/// project (a folder above the model holding `.arp/config.yml`) it is saved in
/// the project's cache under the hash of both files and the program, and read
/// back while they are the same; elsewhere it is assessed each time.
pub fn page_assessment(input: &std::path::Path, model: &std::path::Path) -> Result<Value> {
    let assessed = || -> Result<Value> {
        let report = assess(&load_input(input)?, &load_model(model)?)?;
        Ok(
            json!({"ready":report.ready,"assurance":report.assurance,"summary":report.summary,
            "coverage":report.coverage,"issues":report.issues}),
        )
    };
    let Some(arp) = std::path::absolute(model).ok().and_then(|path| {
        path.ancestors()
            .skip(1)
            .map(|folder| folder.join(".arp"))
            .find(|arp| arp.join("config.yml").is_file())
    }) else {
        return assessed();
    };
    let directory = crate::data::under(&arp, "cache/spec-check")?;
    let key = hash(&encoded(&json!({"program":crate::data::program()?,
        "input":hash(&std::fs::read(input)?),"model":hash(&std::fs::read(model)?)})));
    let path = directory.join(format!("{key}.json"));
    if let Some(saved) = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        return Ok(saved);
    }
    let result = assessed()?;
    // A check that cannot save its assessment still answers.
    if crate::data::replace_cache(&path, &serde_json::to_vec(&result)?).is_ok() {
        crate::data::keep_newest(&directory, 8);
    }
    Ok(result)
}
