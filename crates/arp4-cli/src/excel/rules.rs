//! Conditional formats and data validations of a sheet whose operations
//! shift cells within a band. Such a rule's cells may move apart from the
//! cells its relative formulas name, so each operation splits its ranges
//! into the parts that stay, move or are inserted, as Excel does: the parts
//! whose formulas still read alike stay one rule, and a moved part whose
//! formulas would name other cells becomes a rule of its own, its formulas
//! written as seen from its new place. A rule element is then written once
//! for each rule.
use super::*;

/// A rule's ranges, and its formulas as seen from the top left of the first.
#[derive(Clone)]
struct Rule {
    areas: Vec<Area>,
    formulas: Vec<String>,
}

/// The parts of a range one operation shifting cells in a band makes: those
/// staying, those moving (from, to) and those inserted beside them.
#[derive(Default)]
struct Parts {
    stay: Vec<Area>,
    moved: Vec<(Area, Area)>,
    inserted: Vec<Area>,
}

fn top_left(area: Area) -> (u32, u32) {
    (area.first_column(), area.first_row())
}

/// Splits `area` by the band operation `operation`.
fn parts(area: Area, operation: &StructuralOperation) -> Result<Parts> {
    let (first, last) = operation.band.context("parts of a band operation")?;
    let rows = operation.row_operation();
    let make = |along: (u32, u32), across: (u32, u32)| {
        if rows {
            Area {
                rows: Some(along),
                columns: Some(across),
            }
        } else {
            Area {
                columns: Some(along),
                rows: Some(across),
            }
        }
    };
    let (along, across) = if rows {
        (area.rows, area.columns)
    } else {
        (area.columns, area.rows)
    };
    let mut result = Parts::default();
    let Some((start, end)) = along else {
        result.stay.push(area);
        return Ok(result);
    };
    let max = if rows { MAX_COLUMN } else { MAX_ROW };
    let (low, high) = across.unwrap_or((1, max));
    let (at, count) = (operation.at, operation.count);
    let insertion = operation.insertion();
    let reached = end >= at || insertion && end.saturating_add(1) == at;
    if !reached || high < first || low > last {
        result.stay.push(area);
        return Ok(result);
    }
    let inside = first <= low && high <= last;
    let middle = (low.max(first), high.min(last));
    let before = (start < at).then(|| (start, end.min(at - 1)));
    let (mut moved, mut deleted) = (None, None);
    if insertion {
        if end >= at {
            let from = (start.max(at), end);
            moved = Some((from, (from.0 + count, from.1 + count)));
        }
        // Inserted cells take the format of the cells before them.
        if start < at && at - 1 <= end {
            result.inserted.push(make((at, at + count - 1), middle));
        }
    } else {
        let gone = at + count - 1;
        if start <= gone && end >= at {
            deleted = Some((start.max(at), end.min(gone)));
        }
        if end > gone {
            let from = (start.max(gone + 1), end);
            moved = Some((from, (from.0 - count, from.1 - count)));
        }
    }
    // A range the band reaches part of keeps what it does not move: the
    // cells the band deletes stay covered, and filled by the cells after.
    if !inside && moved.is_none() {
        result.stay.push(area);
        return Ok(result);
    }
    if low < first {
        result.stay.push(make((start, end), (low, first - 1)));
    }
    if high > last {
        result.stay.push(make((start, end), (last + 1, high)));
    }
    if let Some(before) = before {
        result.stay.push(make(before, middle));
    }
    if let (Some(deleted), false) = (deleted, inside) {
        result.stay.push(make(deleted, middle));
    }
    if let Some((from, to)) = moved {
        result.moved.push((make(from, middle), make(to, middle)));
    }
    Ok(result)
}

/// Applies one operation of `sheet` to a rule, giving the rules it becomes.
fn through(rule: Rule, operation: &StructuralOperation, sheet: &str) -> Result<Vec<Rule>> {
    let single = std::slice::from_ref(operation);
    let rewrite = |formula: &str| rewrite_references(formula, Some(sheet), single);
    let anchor = top_left(*rule.areas.first().context("rule without ranges")?);
    if operation.whole() {
        let sqref = rule
            .areas
            .iter()
            .map(|area| area.render())
            .collect::<Result<Vec<_>>>()?
            .join(" ");
        let shift = anchor_shift(&sqref, &[operation])?;
        let Some(mapped) = map_sqref(&sqref, &[operation], true)? else {
            return Ok(vec![]);
        };
        let formulas = rule
            .formulas
            .iter()
            .map(|formula| rewrite(&shift_relative(formula, shift.0, shift.1)?))
            .collect::<Result<_>>()?;
        return Ok(vec![Rule {
            areas: mapped
                .split_whitespace()
                .map(Area::parse)
                .collect::<Result<_>>()?,
            formulas,
        }]);
    }
    // Each part's formulas as seen from its place after the operation, its
    // cells having been at `from` before it.
    let seen_from = |from: (u32, u32)| -> Result<Vec<String>> {
        rule.formulas
            .iter()
            .map(|formula| {
                rewrite(&shift_relative(
                    formula,
                    i64::from(from.1) - i64::from(anchor.1),
                    i64::from(from.0) - i64::from(anchor.0),
                )?)
            })
            .collect()
    };
    let mut stay = vec![];
    let mut moved = vec![];
    let mut inserted = vec![];
    for &area in &rule.areas {
        let mut split = parts(area, operation)?;
        // A range the band holds across that moves alike as a whole is
        // mapped as one, the way whole rows or columns map it.
        if split.inserted.len() + split.moved.len() + split.stay.len() > 1
            && area_held(area, operation)
        {
            let whole = StructuralOperation {
                band: None,
                ..operation.clone()
            };
            if let Some(mapped) = map_area_with_formatting(area, &whole)? {
                // The cell of the range's top left is the one at the mapped
                // range's top left; each moved part must read as it does.
                let corner = top_left(mapped);
                let formulas = seen_from(top_left(area))?;
                let mut alike = true;
                for (from, to) in &split.moved {
                    let (to, from) = (top_left(*to), top_left(*from));
                    let shifted = formulas
                        .iter()
                        .map(|formula| {
                            shift_relative(
                                formula,
                                i64::from(to.1) - i64::from(corner.1),
                                i64::from(to.0) - i64::from(corner.0),
                            )
                        })
                        .collect::<Result<Vec<_>>>();
                    alike &=
                        shifted.is_ok_and(|shifted| shifted == seen_from(from).unwrap_or_default());
                }
                if alike {
                    split = Parts {
                        stay: vec![],
                        moved: vec![(area, mapped)],
                        inserted: vec![],
                    };
                }
            }
        }
        stay.extend(split.stay);
        moved.extend(split.moved);
        inserted.extend(split.inserted);
    }
    // The rule the staying parts keep, or the first moved part takes.
    let (main_areas, main_formulas, rest) = match stay.first() {
        Some(&first) => (stay.clone(), seen_from(top_left(first))?, moved),
        None => match moved.split_first() {
            Some((&(from, to), rest)) => (vec![to], seen_from(top_left(from))?, rest.to_vec()),
            None if !inserted.is_empty() => {
                return Ok(vec![Rule {
                    areas: inserted,
                    formulas: rule.formulas,
                }]);
            }
            None => return Ok(vec![]),
        },
    };
    let mut rules = vec![Rule {
        areas: main_areas,
        formulas: main_formulas,
    }];
    for (from, to) in rest {
        let formulas = seen_from(top_left(from))?;
        let place = top_left(to);
        let joins = |rule: &Rule| -> Result<bool> {
            let origin = top_left(rule.areas[0]);
            let shifted = rule
                .formulas
                .iter()
                .map(|formula| {
                    shift_relative(
                        formula,
                        i64::from(place.1) - i64::from(origin.1),
                        i64::from(place.0) - i64::from(origin.0),
                    )
                })
                .collect::<Result<Vec<_>>>();
            Ok(shifted.is_ok_and(|shifted| shifted == formulas))
        };
        match rules.iter().position(|rule| joins(rule).unwrap_or(false)) {
            Some(index) => rules[index].areas.push(to),
            None => rules.push(Rule {
                areas: vec![to],
                formulas,
            }),
        }
    }
    rules[0].areas.extend(inserted);
    Ok(rules)
}

/// Whether the band of `operation` holds `area` across.
fn area_held(area: Area, operation: &StructuralOperation) -> bool {
    let Some((first, last)) = operation.band else {
        return true;
    };
    let across = if operation.row_operation() {
        area.columns
    } else {
        area.rows
    };
    across.is_some_and(|(low, high)| first <= low && high <= last)
}

fn map_area_with_formatting(area: Area, operation: &StructuralOperation) -> Result<Option<Area>> {
    let sqref = area.render()?;
    map_sqref(&sqref, &[operation], true)?
        .map(|mapped| Area::parse(&mapped))
        .transpose()
}

/// The texts of a rule element its formulas are written in: formula
/// elements, and thresholds that are not plain numbers.
fn formula_places<'a>(element: Node<'a, 'a>) -> Vec<(std::ops::Range<usize>, String, bool)> {
    let mut places = vec![];
    for node in element.descendants().filter(Node::is_element) {
        let name = node.tag_name().name();
        let formula = matches!(
            (node.tag_name().namespace(), name),
            (Some(NS), "formula" | "formula1" | "formula2") | (Some(XM), "f")
        );
        if formula
            && let (Some(text), Some(text_node)) =
                (node.text(), node.children().find(Node::is_text))
        {
            places.push((text_node.range(), text.to_owned(), false));
        }
        if name == "cfvo"
            && let Some(value) = node.attribute("val")
            && value.parse::<f64>().is_err()
            && let Some(attribute) = node.attributes().find(|a| a.name() == "val")
        {
            places.push((attribute.range_value(), value.to_owned(), true));
        }
    }
    places
}

/// The rule element `element` of `sheet` (a conditional format or data
/// validation, its ranges in the `sqref` attribute or an `xm:sqref`
/// element) as `operations` leave it: one element for each rule it becomes,
/// or none when its cells are deleted.
pub(super) fn relocate_rule(
    original: &str,
    element: Node<'_, '_>,
    sheet: &str,
    operations: &[&StructuralOperation],
    others: &[StructuralOperation],
) -> Result<String> {
    let sqref_node = element.children().find(|n| n.has_tag_name((XM, "sqref")));
    let sqref = match sqref_node {
        Some(node) => node.text().unwrap_or_default().to_owned(),
        None => element
            .attribute("sqref")
            .context("rule without sqref")?
            .to_owned(),
    };
    let places = formula_places(element);
    let mut rules = vec![Rule {
        areas: sqref
            .split_whitespace()
            .map(Area::parse)
            .collect::<Result<_>>()?,
        formulas: places.iter().map(|(_, text, _)| text.clone()).collect(),
    }];
    for operation in operations {
        let mut next = vec![];
        for rule in rules {
            next.extend(through(rule, operation, sheet)?);
        }
        rules = next;
    }
    let start = element.range().start;
    let raw = &original[element.range()];
    let local = |range: &std::ops::Range<usize>| range.start - start..range.end - start;
    let mut output = String::new();
    for rule in rules {
        let sqref = rule
            .areas
            .iter()
            .map(|area| area.render())
            .collect::<Result<Vec<_>>>()?
            .join(" ");
        let mut edits = vec![];
        match sqref_node {
            Some(node) => {
                let text = node.children().find(Node::is_text).context("empty sqref")?;
                edits.push((local(&text.range()), sqref));
            }
            None => {
                let (opening, _) = xml_opening(raw)?;
                edits.push((
                    0..opening.len(),
                    replace_xml_attribute(opening, "sqref", &sqref)?,
                ));
            }
        }
        for ((range, _, _), formula) in places.iter().zip(&rule.formulas) {
            // References to other sheets follow their operations too.
            let formula = rewrite_references(formula, Some(sheet), others)?;
            edits.push((local(range), xml_attr(&formula)));
        }
        output.push_str(&splice(raw, edits, "rule")?);
    }
    Ok(output)
}
