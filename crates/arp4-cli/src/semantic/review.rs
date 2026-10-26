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
    // The model is checked once for the packets of every reply.
    let basis = ReviewBasis::new(input, &model, catalog);
    let mut combined = Vec::new();
    if let Some(previous) = previous {
        // The new reviews of each packet, in the order given.
        let mut by_packet = BTreeMap::<&str, Vec<&Review>>::new();
        for review in &reviews {
            by_packet.entry(&review.packet).or_default().push(review);
        }
        for saved in previous.replies {
            let mut review: Review = serde_json::from_value(saved["reply"].clone())?;
            if let Ok(basis) = &basis
                && let Ok(packet) = basis.packet(
                    review.document.as_deref(),
                    review.sheet.as_deref(),
                    review.scope.as_ref(),
                )
                && packet["packet"] == review.packet
            {
                let incoming = by_packet
                    .get(review.packet.as_str())
                    .map_or(&[][..], Vec::as_slice);
                if !incoming.is_empty() {
                    if review.document.is_none() {
                        continue;
                    }
                    let replaced: BTreeSet<&str> = incoming
                        .iter()
                        .flat_map(|r| r.audits.iter().flat_map(|g| g.sources.iter()))
                        .map(String::as_str)
                        .collect();
                    for group in &mut review.audits {
                        group.sources.retain(|s| !replaced.contains(s.as_str()));
                    }
                    review.audits.retain(|g| !g.sources.is_empty());
                    // Only a complete new review of this packet can resolve its saved findings.
                    if packet["sources"]["rows"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|r| r[0].as_str().is_some_and(|s| replaced.contains(s)))
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
    // The sources of each reviewed document, and the IDs of those on each
    // sheet, made once for all the reviews of that document or sheet.
    let mut documents = HashMap::<String, Vec<&Source>>::new();
    let mut sheets = HashMap::<(String, String), HashSet<&str>>::new();
    for (review, reviewer) in combined {
        let basis = match &basis {
            Ok(basis) => basis,
            Err(error) => anyhow::bail!("{error:#}"),
        };
        ensure!(
            basis.packet(
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
        let document = review.document.clone().unwrap();
        let rows = match documents.entry(document.clone()) {
            std::collections::hash_map::Entry::Occupied(rows) => rows.into_mut(),
            std::collections::hash_map::Entry::Vacant(rows) => {
                rows.insert(sources(input, Some(&document))?)
            }
        };
        let on_sheet = review.sheet.as_ref().map(|sheet| {
            &*sheets
                .entry((document.clone(), sheet.clone()))
                .or_insert_with(|| {
                    rows.iter()
                        .filter(|s| s.context.as_ref().is_some_and(|c| &c.sheet == sheet))
                        .map(|s| s.id.as_str())
                        .collect()
                })
        });
        let scope: Option<HashSet<&str>> = review
            .scope
            .as_ref()
            .map(|scope| scope.sources.iter().map(String::as_str).collect());
        for group in review.audits {
            ensure!(
                !group.sources.is_empty() && !group.rationale.trim().is_empty(),
                "empty review group"
            );
            for alias in group.sources {
                ensure!(
                    scope
                        .as_ref()
                        .is_none_or(|scope| scope.contains(alias.as_str())),
                    "audit source outside review scope"
                );
                let evidence = span(&json!(alias), rows)?;
                let source = evidence["source"].as_str().unwrap().to_owned();
                ensure!(
                    on_sheet.is_none_or(|ids| ids.contains(source.as_str())),
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
