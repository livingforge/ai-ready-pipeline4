use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub packet: String,
    pub document: Option<String>,
    #[serde(default)]
    pub sheet: Option<String>,
    #[serde(default)]
    pub scope: Option<ReviewScope>,
    pub audits: Vec<AuditGroup>,
    pub findings: Vec<ReviewFinding>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub message: String,
    pub items: Vec<String>,
    pub action: ReviewAction,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    FieldRepair,
    ScopedRestructure,
    ManualTriage,
    OriginalInspection,
    ValidatorSupport,
    Improvement,
    Review,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditGroup {
    pub sources: Vec<String>,
    pub rationale: String,
}

pub fn apply_reviews(
    input: &Input,
    mut model: Model,
    catalog: &Catalog,
    reviews: Vec<Review>,
    reviewer: &str,
) -> Result<Model> {
    ensure!(!reviewer.trim().is_empty(), "reviewer required");
    let previous = model.review.take();
    model.audits.clear();
    spec::assess(input, &model)?;
    let plan = previous
        .as_ref()
        .map(|r| r.plan.clone())
        .unwrap_or_default();
    plan.validate(input)?;
    let mut combined = Vec::new();
    if let Some(previous) = previous {
        for saved in previous.replies {
            let mut review: Review = serde_json::from_value(saved["reply"].clone())?;
            let current = review_packet_selected(
                input,
                &model,
                catalog,
                review.document.as_deref(),
                review.sheet.as_deref(),
                review.scope.as_ref(),
            );
            if let Ok(packet) = current
                && packet["packet"] == review.packet
            {
                let incoming: Vec<_> = reviews
                    .iter()
                    .filter(|r| r.packet == review.packet)
                    .collect();
                if !incoming.is_empty() {
                    if review.document.is_none() {
                        continue;
                    }
                    let replaced: BTreeSet<_> = incoming
                        .iter()
                        .flat_map(|r| r.audits.iter().flat_map(|g| g.sources.iter()))
                        .collect();
                    for group in &mut review.audits {
                        group.sources.retain(|s| !replaced.contains(s));
                    }
                    review.audits.retain(|g| !g.sources.is_empty());
                    // Only a complete new review of this packet can resolve its saved findings.
                    if packet["sources"]["rows"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|r| replaced.iter().any(|s| r[0] == **s))
                    {
                        review.findings.clear();
                    }
                    if review.audits.is_empty() && review.findings.is_empty() {
                        continue;
                    }
                }
                combined.push((
                    review,
                    saved["reviewer"]
                        .as_str()
                        .context("saved reviewer missing")?
                        .to_owned(),
                ));
            }
        }
    }
    combined.extend(reviews.into_iter().map(|r| (r, reviewer.to_owned())));
    let mut audits = Vec::new();
    let mut seen = BTreeSet::new();
    let mut global = false;
    let mut findings = Vec::new();
    let mut replies = Vec::new();
    for (review, reviewer) in combined {
        ensure!(
            review_packet_selected(
                input,
                &model,
                catalog,
                review.document.as_deref(),
                review.sheet.as_deref(),
                review.scope.as_ref()
            )?["packet"]
                == review.packet,
            "stale review: review the current model"
        );
        replies.push(json!({"reviewer":reviewer,"reply":review}));
        findings.extend(
            review
                .findings
                .iter()
                .map(|f| serde_json::to_value(f).unwrap()),
        );
        if review.document.is_none() {
            ensure!(
                !global && review.audits.is_empty(),
                "global review must occur once with no source audits"
            );
            global = true;
            continue;
        }
        let rows = sources(input, review.document.as_deref())?;
        for group in review.audits {
            ensure!(
                !group.sources.is_empty() && !group.rationale.trim().is_empty(),
                "empty review group"
            );
            for alias in group.sources {
                ensure!(
                    review
                        .scope
                        .as_ref()
                        .is_none_or(|scope| scope.sources.contains(&alias)),
                    "audit source outside review scope"
                );
                let evidence = span(&json!(alias), &rows)?;
                let source = evidence["source"].as_str().unwrap().to_owned();
                ensure!(
                    review
                        .sheet
                        .as_ref()
                        .is_none_or(|sheet| rows.iter().any(|s| s.id == source
                            && s.context.as_ref().is_some_and(|c| &c.sheet == sheet))),
                    "audit source outside review sheet"
                );
                ensure!(seen.insert(source.clone()), "source reviewed twice");
                audits.push(spec::Audit {
                    source,
                    reviewer: reviewer.clone(),
                    rationale: group.rationale.clone(),
                });
            }
        }
    }
    model.audits = audits;
    model.review = Some(spec::ReviewRecord {
        content_hash: spec::review_content_hash(&model),
        plan,
        global,
        findings,
        replies,
    });
    spec::assess(input, &model)?;
    Ok(model)
}
