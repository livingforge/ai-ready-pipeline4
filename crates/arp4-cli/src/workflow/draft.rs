//! Revision-bound edits to an unaccepted reply. Every merged reply is revalidated.
use super::*;

pub(super) fn schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["draft"],
        "properties":{
            "draft":crate::semantic_contract::workflow_reference_schema()["$defs"]["reference"],
            "set":{"type":"object","minProperties":1,"propertyNames":{"pattern":"^/"},"additionalProperties":true},
            "remove":{"type":"array","minItems":1,"uniqueItems":true,"items":{"type":"string","pattern":"^/"}}
        },"anyOf":[{"required":["set"]},{"required":["remove"]}]})
}

fn rejection(code: &str, message: &str) -> anyhow::Error {
    crate::data::Rejection(json!({"code":code,"message":message})).into()
}

fn tokens(pointer: &str) -> Result<Vec<String>> {
    ensure!(
        pointer.starts_with('/'),
        "edit path must be a non-root JSON Pointer"
    );
    pointer[1..]
        .split('/')
        .map(|part| {
            let mut chars = part.chars();
            let mut token = String::new();
            while let Some(c) = chars.next() {
                token.push(if c == '~' {
                    match chars.next() {
                        Some('0') => '~',
                        Some('1') => '/',
                        _ => anyhow::bail!("invalid JSON Pointer escape"),
                    }
                } else {
                    c
                });
            }
            Ok(token)
        })
        .collect()
}

// Apply against the original tree, so deleting array members cannot shift the
// addresses of other edits. None means removal; JSON null remains a real value.
fn edit(value: &mut Value, changes: &[(Vec<String>, Option<Value>)]) -> Result<()> {
    match value {
        Value::Object(object) => {
            let keys: BTreeSet<_> = changes.iter().map(|(path, _)| path[0].clone()).collect();
            for key in keys {
                let nested: Vec<_> = changes
                    .iter()
                    .filter(|(p, _)| p[0] == key)
                    .map(|(p, v)| (p[1..].to_vec(), v.clone()))
                    .collect();
                if nested[0].0.is_empty() {
                    if let Some(v) = &nested[0].1 {
                        object.insert(key, v.clone());
                    } else {
                        ensure!(object.remove(&key).is_some(), "remove path does not exist");
                    }
                } else {
                    edit(
                        object.get_mut(&key).context("edit parent does not exist")?,
                        &nested,
                    )?;
                }
            }
        }
        Value::Array(array) => {
            let mut groups: BTreeMap<usize, Vec<_>> = BTreeMap::new();
            for (path, value) in changes {
                let index: usize = path[0].parse().context("array path requires an index")?;
                ensure!(
                    index.to_string() == path[0] && index < array.len(),
                    "array index out of range or noncanonical"
                );
                groups
                    .entry(index)
                    .or_default()
                    .push((path[1..].to_vec(), value.clone()));
            }
            for (index, nested) in groups.into_iter().rev() {
                if nested[0].0.is_empty() {
                    if let Some(value) = &nested[0].1 {
                        array[index] = value.clone();
                    } else {
                        array.remove(index);
                    }
                } else {
                    edit(&mut array[index], &nested)?;
                }
            }
        }
        _ => anyhow::bail!("edit parent must be an object or array"),
    }
    Ok(())
}

fn merge(mut reply: Value, patch: &Value) -> Result<Value> {
    let errors = crate::semantic_contract::errors(&schema(), patch)?;
    ensure!(errors.is_empty(), "invalid draft patch: {errors:?}");
    let mut changes = Vec::new();
    if let Some(set) = patch["set"].as_object() {
        for (path, value) in set {
            changes.push((tokens(path)?, Some(value.clone())));
        }
    }
    if let Some(remove) = patch["remove"].as_array() {
        for path in remove {
            changes.push((tokens(s(path)?)?, None));
        }
    }
    for (i, (path, _)) in changes.iter().enumerate() {
        ensure!(
            !["packet", "document", "sheet", "scope", "base", "bases"].contains(&path[0].as_str()),
            "task identity cannot be edited"
        );
        for (other, _) in &changes[..i] {
            ensure!(
                !path.starts_with(other) && !other.starts_with(path),
                "overlapping edit paths"
            );
        }
    }
    edit(&mut reply, &changes)?;
    Ok(reply)
}

impl Workflow {
    pub(super) fn draft_info(&self, task: &Task) -> Result<Value> {
        task.draft.as_ref().map_or(Ok(Value::Null), |revision| {
            Ok(json!({"revision":self.transport_ref(revision)?,"schema":schema(),
                "instructions":"previous_reply is the retained draft, not an accepted answer. Use this schema and revision with the same submit/validate-reply command. Omit unused set/remove. JSON Pointers address the original reply root, without a /previous_reply prefix; no overlapping paths. Set replaces a value or adds an object member; replace the containing array to append. All merged content is revalidated; read the new revision after rejection."}))
        })
    }

    pub(super) fn expand_draft(&self, task: &Task, value: Value) -> Result<Value> {
        if value.get("draft").is_none() {
            return Ok(value);
        }
        let revision = task.draft.as_deref().ok_or_else(|| {
            rejection(
                "draft_missing",
                "No editable draft; submit a complete reply.",
            )
        })?;
        if value["draft"].as_str() != Some(self.transport_ref(revision)?.as_str()) {
            return Err(rejection(
                "draft_conflict",
                "Draft changed or belongs to another task; read this task again.",
            ));
        }
        let saved = self.store.json(revision)?;
        ensure!(saved["task"] == task.id, "draft task mismatch");
        ensure!(saved["run"] == json!(self.store.root), "draft run mismatch");
        merge(
            self.compact_payload(self.store.json(s(&saved["reply"])?)?)?,
            &value,
        )
        .map_err(|error| rejection("invalid_draft_patch", &format!("{error:#}")))
    }

    pub(super) fn retain_draft(&mut self, i: usize, value: &Value) -> Result<()> {
        let task = &self.state.tasks[i];
        let reply = self.store.put_json(value)?;
        let revision = self.store.put_json(
            &json!({"task":task.id,"run":self.store.root,"state":self.state.previous,"previous":task.draft,"reply":reply}),
        )?;
        self.state.tasks[i].draft = Some(revision);
        Ok(())
    }

    // A wrong task or ambiguous item identity must never replace a useful draft.
    pub(super) fn check_draft_identity(&self, task: &Task, reply: &Value) -> Result<()> {
        let fields = obj(reply)?;
        let expected = self.complete_reply(task, json!({}))?;
        for (key, value) in obj(&expected)? {
            ensure!(
                fields.get(key) == Some(value),
                "reply {key} does not match task"
            );
        }
        fn unique(value: &Value) -> Result<()> {
            match value {
                Value::Object(object) => {
                    for (name, value) in object {
                        let identity = match name.as_str() {
                            "items" | "changes" => Some("key"),
                            "links" => Some("item"),
                            _ => None,
                        };
                        if let (Some(key), Some(rows)) = (identity, value.as_array()) {
                            let mut seen = BTreeSet::new();
                            for row in rows.iter().filter(|row| row.is_object()) {
                                if let Some(id) = row[key].as_str() {
                                    ensure!(seen.insert(id), "duplicate {name} identity: {id}");
                                }
                            }
                        }
                        unique(value)?;
                    }
                }
                Value::Array(array) => {
                    for value in array {
                        unique(value)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        unique(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_use_original_indices_and_preserve_null_and_escaped_keys() {
        let reply = json!({"items":[{"a/b~":1},2,3,4],"extra":true});
        let patch = json!({"draft":"r0000000000000000000000000.1","set":{"/items/0/a~1b~0":null,"/new":5},"remove":["/items/1","/items/2","/extra"]});
        assert_eq!(
            merge(reply, &patch).unwrap(),
            json!({"items":[{"a/b~":null},4],"new":5})
        );
    }

    #[test]
    fn ambiguous_or_invalid_edits_are_rejected() {
        for changes in [
            json!({"set":{"/items":[],"/items/0":1}}),
            json!({"set":{"/items/0":1},"remove":["/items/0"]}),
            json!({"set":{"/items/01":1}}),
            json!({"set":{"/items/-":1}}),
            json!({"set":{"/items/8":1}}),
            json!({"set":{"/no/child":1}}),
            json!({"set":{"/bad~2":1}}),
            json!({"set":{"":1}}),
            json!({"set":{"/packet":1}}),
            json!({"remove":["/absent"]}),
            json!({"set":{}}),
            json!({"unexpected":true}),
        ] {
            let mut patch = changes;
            patch["draft"] = json!("r0000000000000000000000000.1");
            assert!(merge(json!({"items":[1,2]}), &patch).is_err(), "{patch}");
        }
    }
}
