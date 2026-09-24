//! Explicit identity assignment. A ledger is scoped to one project and must be carried forward.
use crate::{
    data::{encoded, hash},
    specifications::{Kind, Model},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub reviewer: String,
    pub items: BTreeMap<String, Action>,
    #[serde(default)]
    pub retire: BTreeMap<String, String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    New {
        reason: String,
    },
    Retain {
        id: String,
        reason: String,
    },
    Replace {
        predecessors: Vec<String>,
        reason: String,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub kind: Kind,
    pub predecessors: Vec<String>,
    pub reviewer: String,
    pub reason: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub schema_version: u32,
    pub model_hash: String,
    pub entries: BTreeMap<String, Entry>,
    pub active: BTreeSet<String>,
    pub history: Vec<Assignment>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub model_hash: String,
    pub plan: Plan,
    pub aliases: BTreeMap<String, String>,
}
fn prefix(kind: &Kind) -> &'static str {
    match kind {
        Kind::Requirement => "REQ",
        Kind::Specification => "SPEC",
        Kind::Observation => "OBS",
        Kind::Estimate => "EST",
        Kind::Reference => "REF",
    }
}
fn text(s: &str) -> Result<()> {
    ensure!(
        !s.trim().is_empty() && s.trim() == s,
        "empty or untrimmed identity text"
    );
    Ok(())
}
fn fingerprint(model: &Model) -> Result<String> {
    Ok(hash(&encoded(&serde_json::to_value(model)?)))
}
impl Ledger {
    pub fn verify(&self, model: &Model) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.model_hash == fingerprint(model)?,
            "identity ledger/model mismatch"
        );
        let ids: BTreeSet<_> = model.items.iter().map(|i| i.id.clone()).collect();
        ensure!(
            ids.len() == model.items.len() && ids == self.active,
            "identity active set mismatch"
        );
        for (id, entry) in &self.entries {
            let suffix = id
                .strip_prefix(&format!("{}-", prefix(&entry.kind)))
                .context("invalid issued ID prefix")?;
            let number: u64 = suffix.parse()?;
            ensure!(
                number > 0 && *id == format!("{}-{number:06}", prefix(&entry.kind)),
                "invalid issued ID"
            );
            text(&entry.reviewer)?;
            text(&entry.reason)?;
            for predecessor in &entry.predecessors {
                ensure!(
                    predecessor != id && self.entries.contains_key(predecessor),
                    "unknown identity predecessor"
                );
            }
        }
        for item in &model.items {
            ensure!(
                self.entries
                    .get(&item.id)
                    .is_some_and(|e| e.kind == item.kind),
                "unknown ID or changed kind"
            );
            text(
                item.name
                    .as_deref()
                    .context("assigned item requires name")?,
            )?;
        }
        Ok(())
    }
}

pub fn assign(
    mut model: Model,
    plan: Plan,
    previous: Option<(Model, Ledger)>,
) -> Result<(Model, Ledger)> {
    text(&plan.reviewer)?;
    let mut ledger = if let Some((previous, ledger)) = previous {
        ledger.verify(&previous)?;
        ledger
    } else {
        Ledger {
            schema_version: 1,
            model_hash: String::new(),
            entries: BTreeMap::new(),
            active: BTreeSet::new(),
            history: vec![],
        }
    };
    let keys: BTreeSet<_> = model.items.iter().map(|i| i.id.clone()).collect();
    ensure!(
        keys.len() == model.items.len() && keys == plan.items.keys().cloned().collect(),
        "plan must cover every item exactly once"
    );
    let mut aliases = BTreeMap::new();
    let mut retained = BTreeSet::new();
    let mut replaced = BTreeSet::new();
    let mut next = BTreeMap::from([
        ("REQ", 0u64),
        ("SPEC", 0u64),
        ("OBS", 0u64),
        ("EST", 0u64),
        ("REF", 0u64),
    ]);
    for (id, entry) in &ledger.entries {
        let number: u64 = id.rsplit('-').next().unwrap().parse()?;
        let counter = next.get_mut(prefix(&entry.kind)).unwrap();
        *counter = (*counter).max(number);
    }
    // Sorted temporary keys make assignment independent of the JSON array order.
    for (key, action) in &plan.items {
        let item = model.items.iter().find(|i| &i.id == key).unwrap();
        text(
            item.name
                .as_deref()
                .context("assign-ids requires a separate name for every item")?,
        )?;
        let (existing, predecessors, reason) = match action {
            Action::New { reason } => (None, vec![], reason),
            Action::Retain { id, reason } => {
                ensure!(
                    ledger.active.contains(id) && retained.insert(id.clone()),
                    "ID is inactive, unknown, or retained twice"
                );
                ensure!(
                    ledger.entries[id].kind == item.kind,
                    "cannot retain ID across kinds"
                );
                (Some(id.clone()), vec![], reason)
            }
            Action::Replace {
                predecessors,
                reason,
            } => {
                ensure!(
                    !predecessors.is_empty()
                        && predecessors.iter().collect::<BTreeSet<_>>().len() == predecessors.len(),
                    "replacement needs distinct predecessors"
                );
                for id in predecessors {
                    ensure!(
                        ledger.active.contains(id),
                        "replacement predecessor is not active"
                    );
                    replaced.insert(id.clone());
                }
                (None, predecessors.clone(), reason)
            }
        };
        text(reason)?;
        let id = if let Some(id) = existing {
            id
        } else {
            let p = prefix(&item.kind);
            let counter = next.get_mut(p).unwrap();
            *counter = counter.checked_add(1).context("ID sequence exhausted")?;
            let id = format!("{p}-{counter:06}");
            ledger.entries.insert(
                id.clone(),
                Entry {
                    kind: match item.kind {
                        Kind::Requirement => Kind::Requirement,
                        Kind::Specification => Kind::Specification,
                        Kind::Observation => Kind::Observation,
                        Kind::Estimate => Kind::Estimate,
                        Kind::Reference => Kind::Reference,
                    },
                    predecessors,
                    reviewer: plan.reviewer.clone(),
                    reason: reason.clone(),
                },
            );
            id
        };
        aliases.insert(key.clone(), id);
    }
    ensure!(
        retained.is_disjoint(&replaced),
        "an ID cannot be both retained and replaced"
    );
    for (id, reason) in &plan.retire {
        text(reason)?;
        ensure!(
            ledger.active.contains(id) && !retained.contains(id) && !replaced.contains(id),
            "invalid retirement"
        );
    }
    ensure!(
        ledger.active.iter().all(|id| retained.contains(id)
            || replaced.contains(id)
            || plan.retire.contains_key(id)),
        "every previous active ID needs retain, replace, or explicit retire"
    );
    let review_current = model
        .review
        .as_ref()
        .is_some_and(|r| r.content_hash == crate::specifications::review_content_hash(&model));
    let resolve = |id: &str| {
        aliases
            .get(id)
            .cloned()
            .context("reference must use an input model item key")
    };
    for item in &mut model.items {
        item.id = resolve(&item.id)?;
        item.requirements = item
            .requirements
            .iter()
            .map(|s| resolve(s))
            .collect::<Result<_>>()?;
    }
    for decision in &mut model.decisions {
        decision.candidates = decision
            .candidates
            .iter()
            .map(|s| resolve(s))
            .collect::<Result<_>>()?;
        decision.selected = resolve(&decision.selected)?;
    }
    if review_current {
        let content_hash = crate::specifications::review_content_hash(&model);
        let review = model.review.as_mut().unwrap();
        review.content_hash = content_hash;
        // Packet identities refer to temporary item IDs and cannot be reused after assignment.
        review.replies.clear();
    }
    ledger.active = model.items.iter().map(|i| i.id.clone()).collect();
    ledger.model_hash = fingerprint(&model)?;
    ledger.history.push(Assignment {
        model_hash: ledger.model_hash.clone(),
        plan,
        aliases,
    });
    ledger.verify(&model)?;
    Ok((model, ledger))
}
