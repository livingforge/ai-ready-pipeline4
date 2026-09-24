//! Explicit, atomic item addition/splitting/merging, with identity and evidence checks.
use super::*;

pub fn schema() -> Value {
    let reply = crate::semantic_contract::reply_schema();
    let fields = patch::reply_schema();
    let mut result = json!({"type":"object","additionalProperties":false,"required":["bases"],"oneOf":[{"required":["replacements","mapping"],"not":{"anyOf":[{"required":["edits"]},{"required":["defer"]}]}},{"required":["edits","mapping"],"not":{"anyOf":[{"required":["replacements"]},{"required":["defer"]}]}},{"required":["defer"],"not":{"anyOf":[{"required":["edits"]},{"required":["replacements"]},{"required":["mapping"]}]}}],"properties":{
        "bases":{"type":"object","additionalProperties":{"type":"string"}},
        "defer":{"type":"object","additionalProperties":false,"required":["reason","kind"],"properties":{"reason":{"type":"string","minLength":1},"kind":{"enum":["information_required","validator_support","unresolved"]}}},
        "replacements":{"type":"array","minItems":1,"items":{"$ref":"#/$defs/reply"}},
        "edits":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"required":["document","add","remove","changes"],"properties":{
            "document":{"type":"string"},"add":{"type":"array","items":{"$ref":"#/$defs/item"}},
            "remove":{"type":"array","uniqueItems":true,"items":{"type":"string"}},
            "changes":fields["oneOf"][0]["properties"]["changes"],
            "modules":reply["properties"]["modules"],"exclusions":reply["properties"]["exclusions"],"open_issues":reply["properties"]["open_issues"]}}},
        "mapping":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["from","to","reason"],"properties":{
            "from":{"type":"array","uniqueItems":true,"items":{"type":"string"}},"to":{"type":"array","uniqueItems":true,"items":{"type":"string"}},"reason":{"type":"string","minLength":1}}}}
    },"$defs":reply["$defs"]});
    let mut embedded = reply;
    embedded.as_object_mut().unwrap().remove("$defs");
    embedded.as_object_mut().unwrap().remove("$schema");
    result["$defs"]["reply"] = embedded;
    result
}

impl Workflow {
    pub(super) fn restructure_task(&self, findings: &[Value]) -> Result<Option<Task>> {
        let structural = |v: &Value| {
            v["repair"]["action"] == "scoped_restructure"
                || v["finding"]["action"] == "scoped_restructure"
        };
        let eligible = |v: &Value| {
            !self
                .unresolved()
                .iter()
                .any(|entry| entry["code"] == "restructure_deferred" && entry["diagnostic"] == *v)
        };
        if !findings.iter().any(|v| structural(v) && eligible(v)) {
            return Ok(None);
        }
        let selected: Vec<_> = findings
            .iter()
            .filter(|v| structural(v) || v["code"] == "grouped_field_repairs")
            .filter(|v| eligible(v))
            .cloned()
            .collect();
        if selected.is_empty() {
            return Ok(None);
        }
        // Include affected documents and both directions of their relationship dependencies.
        let mut documents = BTreeSet::new();
        for finding in &selected {
            if let Some(doc) = finding["document"].as_str() {
                documents.insert(doc.to_owned());
            }
            for items in [&finding["repair"]["items"], &finding["finding"]["items"]] {
                for id in items
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if let Some((doc, _)) = id.split_once('/') {
                        documents.insert(doc.to_owned());
                    }
                }
            }
        }
        let unscoped = selected.iter().any(|finding| {
            structural(finding)
                && finding["document"].is_null()
                && [&finding["repair"]["items"], &finding["finding"]["items"]]
                    .iter()
                    .all(|items| items.as_array().is_none_or(Vec::is_empty))
        });
        if documents.is_empty() || unscoped {
            documents.extend(obj(&self.state.replies)?.keys().cloned());
        }
        loop {
            let before = documents.clone();
            for (doc, digest) in obj(&self.state.replies)? {
                let reply = self.store.json(s(digest)?)?;
                for item in arr(&reply["items"])? {
                    for link in item["classification"]["requirements"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .chain(
                            item["classification"]["related"]
                                .as_array()
                                .into_iter()
                                .flatten(),
                        )
                    {
                        if let Some((other, _)) = s(link)?.split_once('/') {
                            if before.contains(doc) {
                                documents.insert(other.to_owned());
                            }
                            if before.contains(other) {
                                documents.insert(doc.clone());
                            }
                        }
                    }
                    for evidence in arr(&item["evidence"])? {
                        let alias = evidence
                            .as_str()
                            .or_else(|| evidence["source"].as_str())
                            .unwrap_or("");
                        if let Some((other, _)) = alias.split_once('/')
                            && before.contains(doc)
                        {
                            documents.insert(other.to_owned());
                        }
                    }
                }
            }
            if before == documents {
                break;
            }
        }
        if self.state.round >= self.state.max_rounds {
            return Ok(None);
        }
        let mut replies = Map::new();
        let mut packets = Map::new();
        let mut bases = Map::new();
        for (doc, digest) in obj(&self.state.replies)? {
            if !documents.contains(doc) {
                continue;
            }
            bases.insert(doc.clone(), digest.clone());
            replies.insert(doc.clone(), self.store.json(s(digest)?)?);
            packets.insert(doc.clone(), self.store.json(s(&self.state.packets[doc])?)?);
        }
        let scope = json!({"bases":bases,"replies":replies,"packets":packets,"findings":selected});
        let text = format!(
            "{}\n{}\nReply schema: {}",
            instructions(),
            crate::semantic::PROMPT,
            schema()
        );
        Ok(Some(self.task(
            Stage::Restructure,
            text.as_bytes(),
            json!({"context":self.store.put_json(&scope)?,"bases":bases}),
        )?))
    }

    pub(super) fn restructure_candidate(&self, task: &Task, value: &Value) -> Result<Value> {
        ensure!(
            value["bases"] == task.bases
                && obj(&task.bases)?
                    .iter()
                    .all(|(doc, base)| self.state.replies[doc] == *base),
            "stale restructuring bases"
        );
        let mut next = self.state.replies.clone();
        if let Some(defer) = value.get("defer") {
            ensure!(
                !s(&defer["reason"])?.trim().is_empty(),
                "deferral reason required"
            );
            return Ok(next);
        }
        let mut touched = BTreeSet::new();
        let mut replacements = value["replacements"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for edit in value["edits"].as_array().into_iter().flatten() {
            let doc = s(&edit["document"])?;
            ensure!(
                task.bases.get(doc).is_some(),
                "out-of-scope restructuring document"
            );
            let mut base = self.store.json(s(&task.bases[doc])?)?;
            let old_keys: BTreeSet<String> = arr(&base["items"])?
                .iter()
                .map(|i| s(&i["key"]).map(str::to_owned))
                .collect::<Result<_>>()?;
            let removed: BTreeSet<String> = arr(&edit["remove"])?
                .iter()
                .map(|i| s(i).map(str::to_owned))
                .collect::<Result<_>>()?;
            ensure!(removed.is_subset(&old_keys), "cannot remove unknown item");
            for item in arr(&edit["add"])? {
                ensure!(
                    !old_keys.contains(s(&item["key"])?),
                    "new item reuses an existing key; use changes to retain identity"
                );
            }
            base["items"]
                .as_array_mut()
                .unwrap()
                .extend(arr(&edit["add"])?.clone());
            let allowed: Vec<_> = old_keys.difference(&removed).cloned().collect();
            let scope = json!({"base":patch::fingerprint(&base),"allowed_keys":allowed});
            base = patch::apply(
                &base,
                &scope,
                &json!({"base":scope["base"],"changes":edit["changes"]}),
            )?;
            base["items"]
                .as_array_mut()
                .unwrap()
                .retain(|i| !removed.contains(i["key"].as_str().unwrap()));
            for field in ["modules", "exclusions", "open_issues"] {
                if let Some(value) = edit.get(field) {
                    base[field] = value.clone();
                }
            }
            replacements.push(base);
        }
        for reply in &replacements {
            let doc = s(&reply["document"])?;
            ensure!(
                task.bases.get(doc).is_some() && touched.insert(doc),
                "unknown or duplicate replacement document"
            );
            self.validate_reply(reply)?;
            next[doc] = json!(self.store.put_json(reply)?);
        }
        ensure!(next != self.state.replies, "restructure made no progress");
        let ids = |replies: &Value| -> Result<BTreeSet<String>> {
            let mut ids = BTreeSet::new();
            for (doc, digest) in obj(replies)? {
                for item in arr(&self.store.json(s(digest)?)?["items"])? {
                    ids.insert(format!("{doc}/{}", s(&item["key"])?));
                }
            }
            Ok(ids)
        };
        let old_ids = ids(&self.state.replies)?;
        let new_ids = ids(&next)?;
        let mut from = BTreeSet::new();
        let mut to = BTreeSet::new();
        for mapping in arr(&value["mapping"])? {
            ensure!(
                !s(&mapping["reason"])?.trim().is_empty(),
                "identity mapping reason required"
            );
            ensure!(
                !arr(&mapping["from"])?.is_empty() || !arr(&mapping["to"])?.is_empty(),
                "empty identity mapping"
            );
            for id in arr(&mapping["from"])? {
                let id = s(id)?.to_owned();
                ensure!(
                    old_ids.contains(&id) && from.insert(id),
                    "unknown or duplicate predecessor"
                );
            }
            for id in arr(&mapping["to"])? {
                let id = s(id)?.to_owned();
                ensure!(
                    new_ids.contains(&id) && to.insert(id),
                    "unknown or duplicate successor"
                );
            }
        }
        ensure!(
            old_ids.difference(&new_ids).all(|id| from.contains(id))
                && new_ids.difference(&old_ids).all(|id| to.contains(id)),
            "every added/removed item needs explicit identity mapping"
        );
        let before = self.validate_replies(&self.state.replies)?;
        let after = self.validate_replies(&next)?;
        let missing = |issues: &Value| -> BTreeSet<(String, u64)> {
            issues
                .as_array()
                .unwrap()
                .iter()
                .filter(|i| i["code"] == "uncovered_text")
                .flat_map(|i| {
                    let source = i["source"].as_str().unwrap_or("").to_owned();
                    i["ranges"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(move |range| {
                            let source = source.clone();
                            (range[0].as_u64().unwrap()..range[1].as_u64().unwrap())
                                .map(move |position| (source.clone(), position))
                        })
                })
                .collect()
        };
        let old_missing = missing(&before);
        ensure!(
            missing(&after).is_subset(&old_missing),
            "restructure loses source coverage"
        );
        Ok(next)
    }
}

pub(super) fn instructions() -> &'static str {
    "Restructure claims only to address these findings. Return the supplied JSON schema. bases must copy all supplied references. Prefer sparse edits: each document entry has add (complete new items), remove (old local keys), changes (field patches for retained items), and optional complete modules/exclusions/open_issues. To defer without changing meaning, return only bases and defer: {kind: information_required|validator_support|unresolved, reason: a specific nonblank explanation}. Deferral remains unresolved and is not reissued while these bases stay unchanged. Do not echo unchanged items. Alternatively replacements contains complete replies for changed documents; never supply both edits and replacements. mapping explicitly relates old and new document/key IDs for every added or removed item; additions use empty from, deletions empty to with an evidence-based reason. Update all incoming relationships. Preserve every source claim, condition, exception, quantity and exclusion rationale. Never resolve a conflict by choosing an unsupported winner, weakening a condition, or hiding a quantity in text. Do not remove coverage. Review runs again after adoption."
}
