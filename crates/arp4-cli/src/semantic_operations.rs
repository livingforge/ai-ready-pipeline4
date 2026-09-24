//! Shared semantic operations used by the CLI and persistent workflow.
//! File publication and validation have one implementation for both callers.
use crate::data::{encoded, hash, immutable, read};
use crate::{semantic, specifications as spec};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn assemble(input: &Path, reply: &[PathBuf], actor: &str, out: &Path) -> Result<spec::Report> {
    let input = spec::load_input(input)?;
    let replies = reply
        .iter()
        .map(|p| Ok(serde_json::from_value(read(p, None)?)?))
        .collect::<Result<Vec<_>>>()?;
    let (model, catalog, plan) = semantic::assemble(&input, replies, actor)?;
    let report = spec::assess(&input, &model)?;
    save_bundle(
        out,
        vec![
            ("model.json", serde_json::to_value(&model)?),
            ("catalog.json", serde_json::to_value(catalog)?),
            ("identity-plan.json", plan),
            ("check.json", serde_json::to_value(&report)?),
        ],
    )?;
    Ok(report)
}

pub fn apply_reviews(
    input: &Path,
    model: &Path,
    catalog: &Path,
    review: &[PathBuf],
    reviewer: &str,
    out: &Path,
    plan: Option<spec::ReviewPlan>,
) -> Result<spec::Report> {
    let input = spec::load_input(input)?;
    let reviews = review
        .iter()
        .map(|p| Ok(serde_json::from_value(read(p, None)?)?))
        .collect::<Result<Vec<_>>>()?;
    let mut model = spec::load_model(model)?;
    if let Some(plan) = plan {
        plan.validate(&input)?;
        if let Some(record) = &mut model.review {
            record.plan = plan;
        } else {
            model.review = Some(spec::ReviewRecord {
                content_hash: spec::review_content_hash(&model),
                plan,
                global: false,
                findings: vec![],
                replies: vec![],
            });
        }
    }
    let model = semantic::apply_reviews(
        &input,
        model,
        &serde_json::from_value(read(catalog, None)?)?,
        reviews,
        reviewer,
    )?;
    let report = spec::assess(&input, &model)?;
    let model_value = serde_json::to_value(&model)?;
    let catalog_value = serde_json::to_value(serde_json::from_value::<crate::registry::Catalog>(
        read(catalog, None)?,
    )?)?;
    let receipt = encoded(
        &json!({"model_hash":hash(&encoded(&model_value)),"catalog_hash":hash(&encoded(&catalog_value)),"reviewer":reviewer,"coverage":report.coverage.clone(),"assurance":report.assurance}),
    );
    let receipt_path = PathBuf::from(format!("{}.semantic-review.json", out.display()));
    let model_bytes = encoded(&model_value);
    for (path, bytes) in [(out, &model_bytes), (receipt_path.as_path(), &receipt)] {
        ensure!(
            !path.exists() || fs::read(path)? == *bytes,
            "output already exists with different content: {}",
            path.display()
        );
    }
    immutable(&receipt_path, &receipt)?;
    immutable(out, &model_bytes)?;
    Ok(report)
}

pub fn finalize(
    input: &Path,
    model: &Path,
    catalog: &Path,
    plan: &Path,
    out: &Path,
    registry_out: Option<&Path>,
    project: Option<String>,
) -> Result<()> {
    let input = spec::load_input(input)?;
    let receipt = read(
        &PathBuf::from(format!("{}.semantic-review.json", model.display())),
        None,
    )?;
    let model = spec::load_model(model)?;
    let catalog: crate::registry::Catalog = serde_json::from_value(read(catalog, None)?)?;
    ensure!(
        receipt["model_hash"] == hash(&encoded(&serde_json::to_value(&model)?))
            && receipt["catalog_hash"] == hash(&encoded(&serde_json::to_value(&catalog)?)),
        "reviewed model or catalog changed; repeat affected reviews and review-apply"
    );
    ensure!(
        model.review.as_ref().is_some_and(|r| r.global) && spec::assess(&input, &model)?.ready,
        "reviewed model must pass validation before initial ID assignment"
    );
    let plan: crate::specification_ids::Plan = serde_json::from_value(read(plan, None)?)?;
    ensure!(
        plan.retire.is_empty()
            && plan
                .items
                .values()
                .all(|a| matches!(a, crate::specification_ids::Action::New { .. })),
        "finalize is initial import only; use assign-ids with previous for updates"
    );
    let (model, ledger) = crate::specification_ids::assign(model, plan, None)?;
    let mut catalog = catalog;
    semantic::remap_catalog(&mut catalog, &ledger.history.last().unwrap().aliases)?;
    // Validate the actual registry import, including category/link invariants.
    let model_value = serde_json::to_value(&model)?;
    let ledger_value = serde_json::to_value(&ledger)?;
    let catalog_value = serde_json::to_value(&catalog)?;
    let registry = crate::registry::import(
        input,
        model,
        ledger,
        catalog,
        project.unwrap_or_else(|| "semantic-validation".into()),
    )?;
    let bundle = vec![
        ("model.json", model_value),
        ("model.json.ids.json", ledger_value),
        ("catalog.json", catalog_value),
    ];
    check_bundle(out, &bundle)?;
    if let Some(root) = &registry_out {
        crate::registry::save(&registry, root)?;
    }
    save_bundle(out, bundle)?;
    Ok(())
}

pub fn render(input: &Path, model: &Path, out: &Path, draft: bool) -> Result<PathBuf> {
    let input = spec::load_input(input)?;
    let model = spec::load_model(model)?;
    let text = spec::render(&input, &model, draft)?;
    let report_path = out.with_extension(format!(
        "{}.report.json",
        out.extension().unwrap_or_default().to_string_lossy()
    ));
    let report = encoded(
        &json!({"input_hash":model.input_hash,"model_hash":hash(&encoded(&serde_json::to_value(&model)?)),"validation":spec::assess(&input,&model)?}),
    );
    // Check both destinations before either write, preserving existing artifacts.
    for (path, bytes) in [
        (out, text.as_bytes()),
        (report_path.as_path(), report.as_slice()),
    ] {
        ensure!(
            !path.exists() || fs::read(path)? == bytes,
            "output already exists with different content: {}",
            path.display()
        );
    }
    immutable(&report_path, &report)?;
    immutable(out, text.as_bytes())?;
    Ok(report_path)
}

fn save_bundle(out: &std::path::Path, values: Vec<(&str, Value)>) -> Result<()> {
    check_bundle(out, &values)?;
    let files: Vec<_> = values
        .into_iter()
        .map(|(name, value)| (out.join(name), encoded(&value)))
        .collect();
    for (path, bytes) in files {
        immutable(&path, &bytes)?;
    }
    Ok(())
}

fn check_bundle(out: &std::path::Path, values: &[(&str, Value)]) -> Result<()> {
    for (name, value) in values {
        let path = out.join(name);
        let mut ancestor = path.parent();
        while let Some(parent) = ancestor {
            ensure!(
                !parent.exists() || parent.is_dir(),
                "output parent is not a directory: {}",
                parent.display()
            );
            ancestor = parent.parent();
        }
        ensure!(
            !path.exists() || fs::read(&path)? == encoded(value),
            "output already exists with different content: {}",
            path.display()
        );
    }
    Ok(())
}
