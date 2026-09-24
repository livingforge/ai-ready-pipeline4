use super::*;

pub fn summary(r: &Registry) -> Result<Value> {
    let mut categories = BTreeMap::<String, usize>::new();
    let mut statuses = BTreeMap::<String, usize>::new();
    for e in r.entries.values() {
        *categories
            .entry(serde_json::to_value(&e.category)?.as_str().unwrap().into())
            .or_default() += 1;
        *statuses
            .entry(serde_json::to_value(&e.status)?.as_str().unwrap().into())
            .or_default() += 1;
    }
    let h = &r.manifest.last_change;
    Ok(
        json!({"revision":r.manifest.revision,"base_hash":fingerprint(r)?,"items":r.entries.len(),"categories":categories,"statuses":statuses,"open_issues":r.manifest.open_issues,"conflict_candidates":conflicts(r,false),"last_change":{"actor":h.actor,"changed":h.changed.len(),"impacted":h.impacted.len(),"approved":h.approved.len()}}),
    )
}
pub(super) fn md(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('#', "\\#")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
}
pub fn render(r: &Registry, out: &Path) -> Result<()> {
    validate(r)?;
    let inputs = r
        .inputs
        .iter()
        .map(|(k, v)| Ok((k.clone(), serde_json::from_value::<spec::Input>(v.clone())?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if out.exists() {
        let receipt: BTreeMap<String, String> =
            serde_json::from_value(read(&out.join("generated.json"), None)?)?;
        let actual = crate::data::files(out)?;
        ensure!(actual.len() == receipt.len() + 1, "unmanaged view files");
        for (name, expected) in &receipt {
            ensure!(
                hash(&fs::read(crate::data::under(out, name)?)?) == *expected,
                "view edited; preserve changes before rendering"
            );
        }
    }
    let destination = out;
    let parent = out.parent().context("view has no parent")?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::tempdir_in(parent)?;
    let ready = stage.path().join("ready");
    let out = ready.as_path();
    let mut files = BTreeMap::new();
    let mut index = format!(
        "# {} — 設計・要件台帳\n\n版: {} / 正本ハッシュ: `{}`\n\nこの文書は正本 records/*.json からの生成物です。変更は registry apply で現在版を更新し、過去版はGitで管理します。\n提案は承認済み仕様ではありません。引用・AIレビューは内容の正しさを保証しません。\n\n",
        md(&r.manifest.project),
        r.manifest.revision,
        fingerprint(r)?
    );
    index.push_str(&format!(
        "承認済み: {} / 提案: {} / 廃止: {}\n\n",
        r.entries
            .values()
            .filter(|e| e.status == Status::Approved)
            .count(),
        r.entries
            .values()
            .filter(|e| e.status == Status::Proposed)
            .count(),
        r.entries
            .values()
            .filter(|e| e.status == Status::Retired)
            .count()
    ));
    let candidates = conflicts(r, false);
    if !candidates.is_empty() {
        index.push_str("## 要確認の値の不一致\n\n");
        for group in candidates {
            let links = group
                .iter()
                .map(|id| {
                    format!(
                        "[{}](modules/{}.md#{})",
                        id,
                        r.entries[id].module,
                        id.to_lowercase()
                    )
                })
                .collect::<Vec<_>>();
            index.push_str(&format!("- {}\n", links.join(", ")));
        }
        index.push('\n');
    }
    if !r.manifest.open_issues.is_empty() {
        index.push_str("## 未解決事項\n\n");
        for issue in &r.manifest.open_issues {
            index.push_str(&format!("- {}\n", md(issue)));
        }
        index.push('\n');
    }
    let mut trace = String::from("# 要件と仕様の対応\n\n");
    for req in r
        .entries
        .values()
        .filter(|e| e.category == Category::Requirement && e.status != Status::Retired)
    {
        let specs: Vec<_> = r
            .entries
            .values()
            .filter(|e| e.status != Status::Retired && e.requirements.contains(&req.id))
            .map(|e| {
                format!(
                    "[{}](../modules/{}.md#{})",
                    e.id,
                    e.module,
                    e.id.to_lowercase()
                )
            })
            .collect();
        trace.push_str(&format!(
            "- [{} {}](../modules/{}.md#{}): {}\n",
            req.id,
            md(&req.name),
            req.module,
            req.id.to_lowercase(),
            if specs.is_empty() {
                "対応仕様なし".into()
            } else {
                specs.join(", ")
            }
        ));
    }
    trace.push_str("\n## 対応要件が未登録の仕様\n\n");
    for e in r.entries.values().filter(|e| {
        e.category == Category::Specification
            && e.status != Status::Retired
            && e.requirements.is_empty()
    }) {
        trace.push_str(&format!(
            "- [{} {}](../modules/{}.md#{}): {}\n",
            e.id,
            md(&e.name),
            e.module,
            e.id.to_lowercase(),
            md(e.rationale.as_deref().unwrap_or("未記載"))
        ));
    }
    for (module, title) in &r.manifest.modules {
        let entries: Vec<_> = r.entries.values().filter(|e| &e.module == module).collect();
        if entries.is_empty() {
            continue;
        }
        index.push_str(&format!(
            "- [{}](modules/{}.md)（{}項目）\n",
            md(title),
            module,
            entries.len()
        ));
        let mut body = format!("# {}\n\n[目次](../README.md)\n\n", md(title));
        let mut evidence = format!("# {} — 根拠\n\n", md(title));
        for (heading, status, design) in [
            ("承認済み要件・仕様", Status::Approved, true),
            ("提案中の要件・仕様", Status::Proposed, true),
            ("実績・試算・参考情報", Status::Proposed, false),
            ("廃止", Status::Retired, false),
        ] {
            body.push_str(&format!("## {heading}\n\n"));
            for e in &entries {
                let is_design =
                    matches!(e.category, Category::Requirement | Category::Specification);
                let include = if status == Status::Retired {
                    e.status == Status::Retired
                } else if !design {
                    !is_design && e.status != Status::Retired
                } else {
                    is_design && e.status == status
                };
                if !include {
                    continue;
                }
                body.push_str(&format!(
                    "<a id=\"{}\"></a>\n\n### {} {}\n\n分類: {} / 状態: {}\n\n{}\n\n",
                    e.id.to_lowercase(),
                    e.id,
                    md(&e.name),
                    match e.category {
                        Category::Requirement => "要件",
                        Category::Specification => "仕様",
                        Category::Observation => "実績",
                        Category::Estimate => "試算",
                        Category::Reference => "参考",
                    },
                    match e.status {
                        Status::Proposed => "提案",
                        Status::Approved => "承認済み",
                        Status::Retired => "廃止",
                    },
                    md(&e.statement)
                ));
                body.push_str(&format!(
                    "対象: {} / 属性: {}\n\n条件: {} / 値: {}\n\n",
                    md(&e.subject),
                    md(&e.property),
                    md(e.condition.text()),
                    md(&e.value.display())
                ));
                if matches!(&e.value, AtomicValue::Table { .. }) {
                    body.push_str(&spec::render_table(
                        &e.value,
                        e.evidence
                            .iter()
                            .flat_map(|ev| inputs[&ev.snapshot].sources.iter()),
                    )?);
                }
                if !e.acceptance.is_empty() {
                    body.push_str("受入条件:\n\n");
                    for a in &e.acceptance {
                        body.push_str(&format!("- {}\n", md(a)));
                    }
                } else if is_design {
                    body.push_str("受入条件: 未整備（承認前に設定が必要）\n");
                }
                if e.requirements.is_empty() && e.related.is_empty() {
                    body.push_str("\n関連項目: 未登録\n");
                } else {
                    body.push_str("\n関連:\n\n");
                }
                for id in e.requirements.iter().chain(&e.related) {
                    let t = &r.entries[id];
                    body.push_str(&format!(
                        "- [{} {}]({}.md#{})\n",
                        id,
                        md(&t.name),
                        t.module,
                        id.to_lowercase()
                    ));
                }
                body.push_str(&format!(
                    "\n[根拠・分類理由](../evidence/{}.md#{})\n\n",
                    module,
                    e.id.to_lowercase()
                ));
            }
        }
        for e in entries {
            evidence.push_str(&format!(
                "<a id=\"{}\"></a>\n\n## {} {}\n\n分類理由: {}\n\n元の照合手順: {}\n\n",
                e.id.to_lowercase(),
                e.id,
                md(&e.name),
                md(e.rationale.as_deref().unwrap_or("未記載")),
                md(e.verification
                    .as_deref()
                    .unwrap_or("未記載（検証済みを意味しない）"))
            ));
            if e.evidence.is_empty() {
                evidence.push_str(
                    "直接記述した判断。原資料の引用なし。変更履歴の担当者と理由を参照。\n\n",
                );
            }
            for ev in &e.evidence {
                let input = &inputs[&ev.snapshot];
                let s = input
                    .sources
                    .iter()
                    .find(|s| s.id == ev.span.source)
                    .unwrap();
                let location = s
                    .context
                    .as_ref()
                    .map(|c| format!("{}!{}", c.sheet, c.cell))
                    .unwrap_or(s.location.clone());
                evidence.push_str(&format!(
                    "- {} {} [{}..{}]: {}（入力 `{}`）\n",
                    md(&s.document),
                    md(&location),
                    ev.span.start,
                    ev.span.end,
                    md(&ev.span.quote),
                    ev.snapshot
                ));
            }
            evidence.push('\n');
        }
        files.insert(format!("modules/{module}.md"), body.into_bytes());
        files.insert(format!("evidence/{module}.md"), evidence.into_bytes());
    }
    index.push_str("\n- [要件と仕様の対応](appendices/traceability.md)\n- [現在の変更理由](appendices/change.json)\n- [状態集計](appendices/status.json)\n\n引用原文と判断は正本の evidence/ と archive/ に保存しています。過去版はGitで確認します。\n");
    files.insert("README.md".into(), index.into_bytes());
    files.insert("appendices/traceability.md".into(), trace.into_bytes());
    files.insert(
        "appendices/change.json".into(),
        bytes(&r.manifest.last_change)?,
    );
    files.insert("appendices/status.json".into(), encoded(&summary(r)?));
    fs::create_dir(out)?;
    for folder in ["modules", "evidence", "appendices"] {
        fs::create_dir(out.join(folder))?;
    }
    let receipt: BTreeMap<_, _> = files
        .iter()
        .map(|(name, contents)| (name.clone(), hash(contents)))
        .collect();
    for (name, contents) in files {
        fs::write(out.join(name), contents)?;
    }
    fs::write(out.join("generated.json"), bytes(&receipt)?)?;
    let backup = stage.path().join("previous");
    if destination.exists() {
        fs::rename(destination, &backup)?;
    }
    if let Err(error) = fs::rename(out, destination) {
        if backup.exists()
            && let Err(restore) = fs::rename(&backup, destination)
        {
            let recovery = stage.keep();
            anyhow::bail!(
                "installation failed: {error}; rollback failed: {restore}; recovery data: {}",
                recovery.display()
            );
        }
        return Err(error.into());
    }
    Ok(())
}
