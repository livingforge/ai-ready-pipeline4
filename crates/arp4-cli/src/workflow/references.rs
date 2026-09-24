use super::*;

impl Workflow {
    pub(super) fn references(&self) -> Result<Vec<Value>> {
        let mut references = Vec::new();
        for (path, digest) in obj(&self.state.references)? {
            let text = String::from_utf8(self.store.get(s(digest)?)?)?;
            references.push(json!({"path":path,"text":text}));
        }
        Ok(references)
    }
    pub(super) fn reference_paths(&self) -> Vec<String> {
        self.state
            .references
            .as_object()
            .map(|r| r.keys().cloned().collect())
            .unwrap_or_default()
    }
    /// Reference files are kept by repository-relative path so citations
    /// (`ref:<path>`) stay meaningful outside the run directory.
    pub(super) fn load_references(&self, paths: &[PathBuf]) -> Result<Value> {
        // store.root is <repository>/.arp/work/workflow/<run-id>.
        let root = self.store.root.ancestors().nth(4).map(Path::to_path_buf);
        let mut references = Map::new();
        for path in paths {
            let canonical = dunce::canonicalize(path)
                .with_context(|| format!("reference not found: {}", path.display()))?;
            ensure!(
                !crate::data::is_link(&fs::symlink_metadata(&canonical)?),
                "linked reference path"
            );
            let bytes = fs::read(&canonical)?;
            ensure!(
                bytes.len() <= MAX_REFERENCE_BYTES,
                "reference exceeds {MAX_REFERENCE_BYTES} bytes: {}",
                path.display()
            );
            let text = String::from_utf8(bytes)
                .with_context(|| format!("reference is not UTF-8 text: {}", path.display()))?;
            let name = root
                .as_deref()
                .and_then(|root| canonical.strip_prefix(root).ok().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from(canonical.file_name().unwrap_or_default()));
            let name = name.to_string_lossy().replace('\\', "/");
            ensure!(
                references
                    .insert(name, json!(self.store.put(text.as_bytes())?))
                    .is_none(),
                "duplicate reference: {}",
                path.display()
            );
        }
        Ok(Value::Object(references))
    }
}
