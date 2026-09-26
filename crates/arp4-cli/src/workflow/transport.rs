//! Lossless workflow-local references at the agent boundary. Never inspect source text.
use super::*;

pub(super) fn namespace() -> String {
    let mut id = uuid::Uuid::new_v4().as_u128();
    let mut result = [b'0'; 25];
    for ch in result.iter_mut().rev() {
        *ch = b"0123456789abcdefghijklmnopqrstuvwxyz"[(id % 36) as usize];
        id /= 36;
    }
    String::from_utf8(result.to_vec()).unwrap()
}

// Only protocol identity slots are visited. In particular, item values, quotes,
// source rows, findings and JSON schemas are not searched for hash-like strings.
fn identities(value: &mut Value, visit: &mut impl FnMut(&mut Value) -> Result<()>) -> Result<()> {
    for key in ["packet", "parent_packet", "contract", "base"] {
        if let Some(value) = value.get_mut(key).filter(|v| v.is_string()) {
            visit(value)?;
        }
    }
    if let Some(bases) = value.get_mut("bases").and_then(Value::as_object_mut) {
        for value in bases.values_mut() {
            visit(value)?;
        }
    }
    if let Some(claims) = value.get_mut("claims") {
        identities(claims, visit)?;
    }
    for key in ["replies", "packets"] {
        if let Some(entries) = value.get_mut(key).and_then(Value::as_object_mut) {
            for entry in entries.values_mut() {
                identities(entry, visit)?;
            }
        }
    }
    if let Some(replies) = value.get_mut("replacements").and_then(Value::as_array_mut) {
        for reply in replies {
            identities(reply, visit)?;
        }
    }
    Ok(())
}

impl Workflow {
    pub(super) fn register_transport_refs(&mut self) -> Result<()> {
        let mut digests = BTreeSet::new();
        let mut scanned = Vec::new();
        for task in &self.state.tasks {
            for digest in [&task.packet, &task.context, &task.scope]
                .into_iter()
                .flatten()
                .filter(|digest| !self.state.transport_scanned.contains(*digest))
            {
                identities(&mut self.store.json(digest)?, &mut |v| {
                    digests.insert(s(v)?.to_owned());
                    Ok(())
                })?;
                scanned.push(digest.clone());
            }
            if let Some(draft) = &task.draft {
                digests.insert(draft.clone());
            }
        }
        let known: BTreeSet<_> = self.state.transport_refs.values().cloned().collect();
        for digest in digests.difference(&known) {
            ensure!(
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid transport digest"
            );
            let reference = format!(
                "r{}.{}",
                self.state.transport_namespace,
                self.state.transport_refs.len() + 1
            );
            self.state.transport_refs.insert(reference, digest.clone());
        }
        self.state.transport_scanned.extend(scanned);
        Ok(())
    }

    pub(super) fn transport_ref(&self, digest: &str) -> Result<String> {
        self.state
            .transport_refs
            .iter()
            .find_map(|(reference, known)| (known == digest).then(|| reference.clone()))
            .context("unregistered transport digest")
    }

    pub(super) fn compact_payload(&self, mut value: Value) -> Result<Value> {
        identities(&mut value, &mut |v| {
            *v = json!(self.transport_ref(s(v)?)?);
            Ok(())
        })?;
        Ok(value)
    }

    pub(super) fn expand_transport_reply(&self, task: &Task, value: Value) -> Result<Value> {
        // Resolve against this task's input only, not the entire run's table.
        let mut allowed = BTreeSet::new();
        for digest in [&task.packet, &task.context, &task.scope]
            .into_iter()
            .flatten()
        {
            identities(&mut self.store.json(digest)?, &mut |v| {
                allowed.insert(s(v)?.to_owned());
                Ok(())
            })?;
        }
        self.expand_payload(value, &allowed)
    }

    pub(super) fn expand_payload(
        &self,
        mut value: Value,
        allowed: &BTreeSet<String>,
    ) -> Result<Value> {
        identities(&mut value, &mut |v| {
            let digest = self
                .state
                .transport_refs
                .get(s(v)?)
                .context("unknown workflow reference; copy the reference from read")?;
            ensure!(
                allowed.contains(digest),
                "reference belongs to another task"
            );
            *v = json!(digest);
            Ok(())
        })?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow(root: &Path) -> Workflow {
        let mut state = WorkflowState::test_state();
        state.transport_namespace = namespace();
        Workflow {
            store: Store::open(root).unwrap(),
            state,
            validation_cache: Default::default(),
        }
    }

    fn add_task(flow: &mut Workflow, digest: &str) -> Task {
        let packet = flow.store.put_json(&json!({"packet":digest,"contract":hash(b"contract"),"document":"doc",
            "sources":{"rows":[["s1",digest]]},"items":[{"value":{"packet":digest},"quote":digest}]})).unwrap();
        let task = flow
            .task(
                Stage::Extract,
                b"instructions",
                json!({"packet":packet,"document":"doc"}),
            )
            .unwrap();
        flow.state.tasks.push(task.clone());
        flow.save().unwrap();
        task
    }

    #[test]
    fn references_roundtrip_without_truncating_digests_or_touching_content() {
        let temp = tempfile::tempdir().unwrap();
        let mut flow = workflow(temp.path());
        // Deliberately identical prefixes: distinct digests must stay distinct.
        let first = format!("{}0", "a".repeat(63));
        let second = format!("{}1", "a".repeat(63));
        let task = add_task(&mut flow, &first);
        let other = add_task(&mut flow, &second);
        let original = flow.artifact(&task.packet).unwrap();
        let compact = flow.compact_payload(original.clone()).unwrap();
        assert!(s(&compact["packet"]).unwrap().len() < 32);
        assert_ne!(
            compact["packet"],
            flow.compact_payload(flow.artifact(&other.packet).unwrap())
                .unwrap()["packet"]
        );
        assert_eq!(compact["sources"], original["sources"]);
        assert_eq!(compact["items"], original["items"]);
        assert_eq!(
            flow.expand_transport_reply(&task, compact).unwrap(),
            original
        );
        assert!(
            flow.expand_transport_reply(&task, json!({"packet":first}))
                .is_err()
        );
        assert!(
            flow.expand_transport_reply(
                &task,
                json!({"packet":flow.transport_ref(&second).unwrap()})
            )
            .is_err()
        );
        let refs = flow.state.transport_refs.clone();
        flow.save().unwrap();
        assert_eq!(flow.state.transport_refs, refs);
        assert!(WorkflowState::decode(serde_json::to_value(&flow.state).unwrap()).is_ok());
    }

    #[test]
    fn references_from_another_run_are_rejected_even_for_identical_tasks() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut first = workflow(a.path());
        let mut second = workflow(b.path());
        let digest = hash(b"same packet");
        let task = add_task(&mut first, &digest);
        let other = add_task(&mut second, &digest);
        assert_eq!(task.id, other.id);
        let reply = first.compact_payload(json!({"packet":digest})).unwrap();
        assert!(second.expand_transport_reply(&other, reply).is_err());
    }
}
