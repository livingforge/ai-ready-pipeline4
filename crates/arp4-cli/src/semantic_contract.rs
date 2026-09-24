//! Agent-facing contracts share the model's value/condition definitions.
use crate::{
    data::{encoded, hash},
    specifications as spec,
};
use anyhow::{Context, Result};
use serde_json::{Value, json};

pub fn reply_schema() -> Value {
    let mut schema = spec::schema();
    schema["$defs"]["span"] = json!({"oneOf":[
        {"type":"string","pattern":"^s[1-9][0-9]*$"},
        {"type":"object","additionalProperties":false,"required":["source","quote"],"properties":{
            "source":{"type":"string","pattern":"^(?:[^/]+/)?s[1-9][0-9]*$"},
            "quote":{"type":"string","minLength":1},"occurrence":{"type":"integer","minimum":1}}}
    ]});
    schema["$defs"]["classification"] = json!({"type":"object","additionalProperties":false,
        "required":["category","module"],"properties":{
        "category":{"enum":["requirement","specification","observation","estimate","reference"]},
        "module":{"type":"string","pattern":"^[a-z0-9-]+$"},
        "requirements":{"type":"array","uniqueItems":true,"items":{"$ref":"#/$defs/text"}},
        "related":{"type":"array","uniqueItems":true,"items":{"$ref":"#/$defs/text"}},
        "reason":{"$ref":"#/$defs/text"}}});
    let item = schema["$defs"]["item"].as_object_mut().unwrap();
    item.insert(
        "then".into(),
        json!({"properties":{"classification":{"properties":{
            "category":{"$ref":"#/$defs/table_category"}
        }}}}),
    );
    item.insert(
        "required".into(),
        json!([
            "key",
            "name",
            "section",
            "subject",
            "property",
            "condition",
            "value",
            "evidence",
            "classification"
        ]),
    );
    let props = item["properties"].as_object_mut().unwrap();
    // Additional context only; quantity/condition selectors are merged by the CLI.
    props.get_mut("evidence").unwrap()["minItems"] = json!(0);
    for key in ["id", "kind", "requirements"] {
        props.remove(key);
    }
    props.insert(
        "key".into(),
        json!({"type":"string","pattern":"^[A-Za-z0-9_-]+$"}),
    );
    props.insert(
        "classification".into(),
        json!({"$ref":"#/$defs/classification"}),
    );
    schema["required"] = json!([
        "packet",
        "document",
        "modules",
        "items",
        "exclusions",
        "open_issues"
    ]);
    schema["properties"] = json!({
        "packet":{"type":"string"},"document":{"type":"string","minLength":1},
        "modules":{"type":"object","propertyNames":{"pattern":"^[a-z0-9-]+$"},"additionalProperties":{"$ref":"#/$defs/text"}},
        "items":{"type":"array","items":{"$ref":"#/$defs/item"}},
        "exclusions":{"type":"array","items":{"$ref":"#/$defs/exclusion"}},
        "open_issues":{"type":"array","items":{"$ref":"#/$defs/text"}}
    });
    for unused in [
        "audit",
        "decision",
        "review_plan",
        "review_record",
        "review_reply",
        "review_finding",
    ] {
        schema["$defs"].as_object_mut().unwrap().remove(unused);
    }
    schema
}

pub fn review_schema() -> Value {
    let model = spec::schema();
    let mut schema = model["$defs"]["review_reply"].clone();
    schema["$defs"] =
        json!({"text":model["$defs"]["text"],"review_finding":model["$defs"]["review_finding"]});
    schema
}

pub fn errors(schema: &Value, value: &Value) -> Result<Vec<Value>> {
    let validator = jsonschema::validator_for(schema)?;
    Ok(validator.iter_errors(value).map(schema_violation).collect())
}

/// Messages name the rule, not the offending value: `path` locates it and the agent
/// already holds the reply. A short scalar is echoed as `value` since it is cheaper
/// than a second lookup; objects and long strings are never inlined.
fn schema_violation(error: jsonschema::ValidationError<'_>) -> Value {
    let mut diagnostic = json!({"code":"schema_violation","path":error.instance_path().to_string(),
        "message":error.masked().to_string()});
    let instance = error.instance();
    if !instance.is_object() && !instance.is_array() && instance.to_string().len() <= 120 {
        diagnostic["value"] = instance.clone().into_owned();
    }
    diagnostic
}

/// Quality findings for a structurally validated extraction, before global assembly.
/// These are not schema failures; unsupported expressions must remain representable.
pub fn quantity_findings(input: &spec::Input, reply: &Value) -> Result<Vec<Value>> {
    Ok(reply_report(input, reply)?
        .issues
        .into_iter()
        .filter(|i| i["code"].as_str().is_some_and(|c| c.contains("quantity")))
        .collect())
}

/// Machine assessment of a validated reply. Source coverage is not a semantic audit.
pub fn reply_report(input: &spec::Input, reply: &Value) -> Result<spec::Report> {
    let doc = reply["document"].as_str().context("document required")?;
    let rows = crate::semantic::sources(input, Some(doc))?;
    let mut items = Vec::new();
    for original in reply["items"].as_array().context("items required")? {
        let mut item = original.clone();
        item["id"] = json!(format!(
            "{doc}/{}",
            original["key"].as_str().context("key required")?
        ));
        item["kind"] = original["classification"]["category"].clone();
        item["requirements"] = json!([]);
        item.as_object_mut().unwrap().remove("key");
        item.as_object_mut().unwrap().remove("classification");
        crate::semantic::expand_item(input, &mut item, &rows)?;
        items.push(item);
    }
    let exclusions = reply["exclusions"].as_array().context("exclusions required")?.iter()
        .map(|entry| -> Result<Value> {
            Ok(json!({"evidence":crate::semantic::span(&entry["evidence"], &rows)?,"reason":entry["reason"]}))
        }).collect::<Result<Vec<_>>>()?;
    let model: spec::Model = serde_json::from_value(json!({"schema_version":1,
        "input_hash":hash(&encoded(&serde_json::to_value(input)?)),"items":items,
        "exclusions":exclusions,"audits":[],"decisions":[]}))?;
    spec::assess(input, &model)
}

/// Validate each item separately so one malformed citation does not hide other errors.
pub fn diagnostics(input: &spec::Input, reply: &Value) -> Result<Vec<Value>> {
    static VALIDATOR: std::sync::LazyLock<jsonschema::Validator> = std::sync::LazyLock::new(|| {
        jsonschema::validator_for(&reply_schema()).expect("embedded reply schema")
    });
    let mut result: Vec<Value> = VALIDATOR.iter_errors(reply).map(schema_violation).collect();
    if !result.is_empty() {
        return Ok(result);
    }
    let doc = reply["document"].as_str().unwrap();
    let packet = crate::semantic::packet(input, doc)?;
    if reply["packet"] != packet["packet"] {
        result.push(json!({"code":"stale_packet","path":"/packet","message":"Use the current extraction packet"}));
        return Ok(result);
    }
    let rows = crate::semantic::sources(input, Some(doc))?;
    let items = reply["items"].as_array().unwrap();
    let input_hash = hash(&encoded(&serde_json::to_value(input)?));
    let mut expanded = Vec::new();
    let mut keys = std::collections::BTreeSet::new();
    for (index, original) in items.iter().enumerate() {
        let key = original["key"].as_str().unwrap();
        // Classification checks do not depend on citation expansion. Keep collecting
        // them so fixing a module does not reveal a second, previously hidden error.
        let c = &original["classification"];
        let mut report = |message: String| {
            result.push(json!({"code":"invalid_item","item":format!("{doc}/{key}"),
                "path":format!("/items/{index}"),"message":message}));
        };
        if !keys.insert(key) {
            report("duplicate item key".into());
        }
        if reply["modules"]
            .get(c["module"].as_str().unwrap())
            .is_none()
        {
            report("unknown module".into());
        }
        if c["category"] != "specification"
            && c["requirements"].as_array().is_some_and(|v| !v.is_empty())
        {
            report("only specifications implement requirements".into());
        }
        for field in ["requirements", "related"] {
            for target in c[field].as_array().into_iter().flatten() {
                let target = target.as_str().unwrap();
                if target.contains('/') && !target.starts_with(&format!("{doc}/")) {
                    continue;
                }
                let local = target.strip_prefix(&format!("{doc}/")).unwrap_or(target);
                let other = items.iter().find(|i| i["key"] == local);
                if local == key || other.is_none() {
                    report(format!("unknown or self relationship: {target}"));
                } else if field == "requirements"
                    && other.is_some_and(|item| item["classification"]["category"] != "requirement")
                {
                    report("requirement target has wrong category".into());
                }
            }
        }
        let checked = (|| -> Result<()> {
            let mut item = original.clone();
            let o = item.as_object_mut().unwrap();
            o.remove("key");
            o.remove("classification");
            o.insert("id".into(), json!(format!("{doc}/{key}")));
            o.insert("kind".into(), c["category"].clone());
            o.insert("requirements".into(), json!([]));
            crate::semantic::expand_item(input, &mut item, &rows)?;
            expanded.push((index, item));
            Ok(())
        })();
        if let Err(error) = checked {
            result.push(json!({"code":"invalid_item","item":format!("{doc}/{key}"),"path":format!("/items/{index}"),"message":format!("{error:#}")}));
        }
    }
    // Most replies are valid. Assess the entire batch once; only malformed batches
    // need the original item-by-item diagnostics. Report-level findings are still
    // routed by assembly, not promoted to extraction errors here.
    let assess_items = |items: Vec<Value>| -> Result<()> {
        let model: spec::Model = serde_json::from_value(json!({"schema_version":1,
            "input_hash":input_hash,"items":items,"exclusions":[],"audits":[],"decisions":[]}))?;
        spec::assess(input, &model)?;
        Ok(())
    };
    if !expanded.is_empty()
        && assess_items(expanded.iter().map(|(_, item)| item.clone()).collect()).is_err()
    {
        for (index, item) in &expanded {
            if let Err(error) = assess_items(vec![item.clone()]) {
                let key = items[*index]["key"].as_str().unwrap();
                result.push(json!({"code":"invalid_item","item":format!("{doc}/{key}"),"path":format!("/items/{index}"),"message":format!("{error:#}")}));
            }
        }
    }
    let mut dispositions = Vec::new();
    for (index, item) in &expanded {
        collect_spans(
            item,
            &format!("/items/{index}"),
            Some(&item["id"]),
            &mut dispositions,
        );
    }
    for (index, exclusion) in reply["exclusions"].as_array().unwrap().iter().enumerate() {
        let path = format!("/exclusions/{index}/evidence");
        match crate::semantic::span(&exclusion["evidence"], &rows) {
            Err(error) => result.push(json!({"code":"invalid_exclusion","document":doc,"path":path,"message":format!("{error:#}")})),
            Ok(span) => {
                for (other, other_path, item) in &dispositions {
                    if span["source"] == other["source"]
                        && span["start"].as_u64() < other["end"].as_u64()
                        && other["start"].as_u64() < span["end"].as_u64()
                    {
                        result.push(json!({"code":"excluded_span_overlap","document":doc,
                            "path":path,"source":exclusion["evidence"],"span":span,
                            "conflicting_path":other_path,"conflicting_item":item,"conflicting_span":other,
                            "message":"Excluded text overlaps item evidence or another exclusion. Use non-overlapping exact quotes; a source string selects the entire source."}));
                    }
                }
                dispositions.push((span, path, None));
            }
        }
    }
    Ok(result)
}

// Inspect expanded spans, including condition and quantity bases and table cells.
fn collect_spans(
    value: &Value,
    path: &str,
    item: Option<&Value>,
    spans: &mut Vec<(Value, String, Option<Value>)>,
) {
    match value {
        Value::Object(fields)
            if fields.contains_key("source")
                && fields.contains_key("start")
                && fields.contains_key("end") =>
        {
            spans.push((value.clone(), path.to_owned(), item.cloned()));
        }
        Value::Object(fields) => {
            for (key, child) in fields {
                collect_spans(child, &format!("{path}/{key}"), item, spans);
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                collect_spans(child, &format!("{path}/{index}"), item, spans);
            }
        }
        _ => {}
    }
}

/// Persisted workflow reference table and its agent-facing identifier shape.
pub fn workflow_reference_schema() -> Value {
    serde_json::from_str(include_str!(
        "../../../contracts/agent-reference-schema.json"
    ))
    .unwrap()
}

pub fn workflow_report_schemas() -> Value {
    serde_json::from_str(include_str!("../../../contracts/workflow-reports.json"))
        .expect("embedded workflow report schemas")
}

/// Apply transport identity types without changing the semantic payload schema.
pub fn workflow_reply_schema(mut value: Value) -> Value {
    let reference = workflow_reference_schema()["$defs"]["reference"].clone();
    fn rewrite(value: &mut Value, reference: &Value) {
        if let Some(properties) = value.get_mut("properties").and_then(Value::as_object_mut) {
            for key in ["packet", "base"] {
                if let Some(slot) = properties.get_mut(key) {
                    *slot = reference.clone();
                }
            }
            if let Some(bases) = properties.get_mut("bases") {
                bases["additionalProperties"] = reference.clone();
            }
        }
        if let Some(reply) = value.pointer_mut("/$defs/reply") {
            rewrite(reply, reference);
        }
        if let Some(branches) = value.get_mut("oneOf").and_then(Value::as_array_mut) {
            for branch in branches {
                rewrite(branch, reference);
            }
        }
    }
    rewrite(&mut value, &reference);
    value
}
