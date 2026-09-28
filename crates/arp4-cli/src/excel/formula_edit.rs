//! New formula text for cells that already hold a formula. The text is the
//! file's own notation, as the extraction shows it: English function names,
//! commas between arguments and `_xlfn.` before functions newer than Excel 2007.
use super::*;

/// Functions added after Excel 2007. The file writes them with `_xlfn.`;
/// without it Excel reads an unknown name and shows #NAME?.
const FUTURE_FUNCTIONS: &[&str] = &[
    "ACOT",
    "ACOTH",
    "AGGREGATE",
    "ANCHORARRAY",
    "ARABIC",
    "ARRAYTOTEXT",
    "BASE",
    "BETA.DIST",
    "BETA.INV",
    "BINOM.DIST",
    "BINOM.DIST.RANGE",
    "BINOM.INV",
    "BITAND",
    "BITLSHIFT",
    "BITOR",
    "BITRSHIFT",
    "BITXOR",
    "BYCOL",
    "BYROW",
    "CEILING.MATH",
    "CEILING.PRECISE",
    "CHISQ.DIST",
    "CHISQ.DIST.RT",
    "CHISQ.INV",
    "CHISQ.INV.RT",
    "CHISQ.TEST",
    "CHOOSECOLS",
    "CHOOSEROWS",
    "COMBINA",
    "CONCAT",
    "CONFIDENCE.NORM",
    "CONFIDENCE.T",
    "COT",
    "COTH",
    "COVARIANCE.P",
    "COVARIANCE.S",
    "CSC",
    "CSCH",
    "DAYS",
    "DECIMAL",
    "DROP",
    "ENCODEURL",
    "ERF.PRECISE",
    "ERFC.PRECISE",
    "EXPAND",
    "EXPON.DIST",
    "F.DIST",
    "F.DIST.RT",
    "F.INV",
    "F.INV.RT",
    "F.TEST",
    "FIELDVALUE",
    "FILTER",
    "FILTERXML",
    "FLOOR.MATH",
    "FLOOR.PRECISE",
    "FORECAST.ETS",
    "FORECAST.ETS.CONFINT",
    "FORECAST.ETS.SEASONALITY",
    "FORECAST.ETS.STAT",
    "FORECAST.LINEAR",
    "FORMULATEXT",
    "GAMMA",
    "GAMMA.DIST",
    "GAMMA.INV",
    "GAMMALN.PRECISE",
    "GAUSS",
    "GROUPBY",
    "HSTACK",
    "HYPGEOM.DIST",
    "IFNA",
    "IFS",
    "IMAGE",
    "IMCOSH",
    "IMCOT",
    "IMCSC",
    "IMCSCH",
    "IMSEC",
    "IMSECH",
    "IMSINH",
    "IMTAN",
    "ISFORMULA",
    "ISO.CEILING",
    "ISOMITTED",
    "ISOWEEKNUM",
    "LAMBDA",
    "LET",
    "LOGNORM.DIST",
    "LOGNORM.INV",
    "MAKEARRAY",
    "MAP",
    "MAXIFS",
    "MINIFS",
    "MODE.MULT",
    "MODE.SNGL",
    "MUNIT",
    "NEGBINOM.DIST",
    "NETWORKDAYS.INTL",
    "NORM.DIST",
    "NORM.INV",
    "NORM.S.DIST",
    "NORM.S.INV",
    "NUMBERVALUE",
    "PDURATION",
    "PERCENTILE.EXC",
    "PERCENTILE.INC",
    "PERCENTOF",
    "PERCENTRANK.EXC",
    "PERCENTRANK.INC",
    "PERMUTATIONA",
    "PHI",
    "PIVOTBY",
    "POISSON.DIST",
    "QUARTILE.EXC",
    "QUARTILE.INC",
    "RANDARRAY",
    "RANK.AVG",
    "RANK.EQ",
    "REDUCE",
    "REGEXEXTRACT",
    "REGEXREPLACE",
    "REGEXTEST",
    "RRI",
    "SCAN",
    "SEC",
    "SECH",
    "SEQUENCE",
    "SHEET",
    "SHEETS",
    "SINGLE",
    "SKEW.P",
    "SORT",
    "SORTBY",
    "STDEV.P",
    "STDEV.S",
    "STOCKHISTORY",
    "SWITCH",
    "T.DIST",
    "T.DIST.2T",
    "T.DIST.RT",
    "T.INV",
    "T.INV.2T",
    "T.TEST",
    "TAKE",
    "TEXTAFTER",
    "TEXTBEFORE",
    "TEXTJOIN",
    "TEXTSPLIT",
    "TOCOL",
    "TOROW",
    "TRIMRANGE",
    "UNICHAR",
    "UNICODE",
    "UNIQUE",
    "VALUETOTEXT",
    "VAR.P",
    "VAR.S",
    "VSTACK",
    "WEBSERVICE",
    "WEIBULL.DIST",
    "WORKDAY.INTL",
    "WRAPCOLS",
    "WRAPROWS",
    "XLOOKUP",
    "XMATCH",
    "XOR",
    "Z.TEST",
];

/// Functions whose result spills over neighboring cells or that bind names.
/// Excel stores them with array metadata or `_xlpm.` parameter names that a
/// formula text alone lacks; written without them, Excel reads a different
/// formula (`=@FILTER(...)`).
const EXCEL_ONLY_FUNCTIONS: &[&str] = &[
    "ANCHORARRAY",
    "BYCOL",
    "BYROW",
    "CHOOSECOLS",
    "CHOOSEROWS",
    "DROP",
    "EXPAND",
    "FILTER",
    "GROUPBY",
    "HSTACK",
    "ISOMITTED",
    "LAMBDA",
    "LET",
    "MAKEARRAY",
    "MAP",
    "PIVOTBY",
    "RANDARRAY",
    "REDUCE",
    "SCAN",
    "SEQUENCE",
    "SORT",
    "SORTBY",
    "TAKE",
    "TEXTSPLIT",
    "TOCOL",
    "TOROW",
    "TRIMRANGE",
    "UNIQUE",
    "VSTACK",
    "WRAPCOLS",
    "WRAPROWS",
];

/// Error values a formula can name.
const ERRORS: &[&str] = &[
    "#NULL!",
    "#DIV/0!",
    "#VALUE!",
    "#REF!",
    "#NAME?",
    "#NUM!",
    "#N/A",
    "#GETTING_DATA",
    "#SPILL!",
    "#CALC!",
    "#FIELD!",
    "#BLOCKED!",
    "#CONNECT!",
    "#BUSY!",
    "#UNKNOWN!",
];

/// Excel's limit on the length of a formula.
const MAX_FORMULA_CHARACTERS: usize = 8192;

/// The byte after the literal opened at `start` by `quote`, where a doubled
/// quote stands for the quote itself.
fn literal_end(text: &str, start: usize, quote: char) -> Option<usize> {
    let mut index = start + 1;
    loop {
        let offset = text[index..].find(quote)?;
        index += offset + 1;
        if !text[index..].starts_with(quote) {
            return Some(index);
        }
        index += 1;
    }
}

/// Checks formula text written without its leading `=`: literals, brackets
/// and parentheses close, and the functions use the notation Excel reads back.
pub fn check_formula(formula: &str) -> Result<()> {
    ensure!(!formula.trim().is_empty(), "the formula is empty");
    ensure!(
        !formula.starts_with('='),
        "write the formula with a single leading ="
    );
    ensure!(
        formula.chars().count() <= MAX_FORMULA_CHARACTERS,
        "the formula is longer than Excel's {MAX_FORMULA_CHARACTERS} characters"
    );
    let mut parentheses = 0usize;
    let mut braces = 0usize;
    let mut index = 0;
    while let Some(c) = formula[index..].chars().next() {
        match c {
            '"' | '\'' => {
                index = literal_end(formula, index, c).with_context(|| {
                    if c == '"' {
                        "a string literal is not closed with \"".to_owned()
                    } else {
                        "a sheet name is not closed with '".to_owned()
                    }
                })?;
                continue;
            }
            '[' => {
                // Structured references and external workbooks nest brackets.
                let mut depth = 0;
                let mut end = None;
                for (offset, ch) in formula[index..].char_indices() {
                    match ch {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(index + offset + 1);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                index = end.context("a [ is not closed with ]")?;
                continue;
            }
            '(' => parentheses += 1,
            ')' => {
                parentheses = parentheses
                    .checked_sub(1)
                    .context("a ) has no matching (")?;
            }
            '{' => braces += 1,
            '}' => braces = braces.checked_sub(1).context("a } has no matching {")?,
            '#' => {
                let rest = formula[index..].to_ascii_uppercase();
                let error = ERRORS
                    .iter()
                    .find(|error| rest.starts_with(**error))
                    .context("a spill reference (#) is written as _xlfn.ANCHORARRAY in the file, which needs Excel's array metadata; enter this formula in Excel")?;
                index += error.len();
                continue;
            }
            '@' => bail!(
                "the implicit intersection operator @ is written as _xlfn.SINGLE in the file; write the formula without @ or enter it in Excel"
            ),
            c if c.is_alphabetic() || c == '_' || c == '\\' => {
                let length = formula[index..]
                    .char_indices()
                    .find(|(_, ch)| !(ch.is_alphanumeric() || matches!(ch, '_' | '.' | '\\')))
                    .map_or(formula.len() - index, |(offset, _)| offset);
                let name = &formula[index..index + length];
                index += length;
                if formula[index..].starts_with('(') {
                    check_function(name)?;
                }
                continue;
            }
            _ => {}
        }
        index += c.len_utf8();
    }
    ensure!(parentheses == 0, "a ( is not closed with )");
    ensure!(braces == 0, "a {{ is not closed with }}");
    Ok(())
}

fn check_function(name: &str) -> Result<()> {
    let upper = name.to_ascii_uppercase();
    let prefixed = upper.starts_with("_XLFN.");
    let bare = upper
        .trim_start_matches("_XLFN.")
        .trim_start_matches("_XLWS.");
    ensure!(
        !EXCEL_ONLY_FUNCTIONS.contains(&bare),
        "{name} spills its result or binds names, which Excel stores with metadata a formula text lacks; enter this formula in Excel"
    );
    ensure!(
        prefixed || !FUTURE_FUNCTIONS.contains(&bare),
        "{name} is newer than Excel 2007, so the file names it _xlfn.{name}; write _xlfn.{name}( so that Excel does not show #NAME?"
    );
    Ok(())
}

/// Replaces the formula of each cell of `edits` (address to formula text
/// without `=`) and drops its cached result. Shared formulas on the sheet are
/// expanded first, since the edited cell may be the one others derive from.
pub(super) fn edit_formulas(
    source: &str,
    sheet: &str,
    edits: &BTreeMap<String, String>,
) -> Result<String> {
    let cells = |doc: &Document<'_>| -> Vec<(String, bool)> {
        doc.descendants()
            .filter(|n| n.has_tag_name((NS, "c")))
            .filter_map(|c| {
                let address = c.attribute("r")?;
                edits.contains_key(address).then(|| {
                    (
                        address.to_owned(),
                        child(c, "f").is_some_and(|f| f.attribute("t") == Some("shared")),
                    )
                })
            })
            .collect()
    };
    let shared = cells(&xml(source.as_bytes())?)
        .iter()
        .any(|(_, shared)| *shared);
    let unshared = if shared {
        unshare_formulas(source, sheet, &Moves::new(&[], BTreeMap::new()), true)?
    } else {
        None
    };
    let text = unshared.as_deref().unwrap_or(source);
    let doc = xml(text.as_bytes())?;
    let type_attribute = attribute_pattern("t")?;
    let mut replacements = vec![];
    for cell in doc.descendants().filter(|n| {
        n.has_tag_name((NS, "c")) && n.parent().is_some_and(|p| p.has_tag_name((NS, "row")))
    }) {
        let address = cell.attribute("r").context("missing cell address")?;
        let Some(formula) = edits.get(address) else {
            continue;
        };
        let current = child(cell, "f").with_context(|| {
            format!("{sheet}!{address} holds no formula; only existing formulas can be replaced")
        })?;
        ensure!(
            !matches!(current.attribute("t"), Some("array" | "dataTable"))
                && cell.attribute("cm").is_none(),
            "{sheet}!{address} holds an array formula, spill or data table, whose range Excel keeps with the formula; edit it in Excel"
        );
        let (opening, _) = xml_opening(&text[cell.range()])?;
        let tag = element_tag(opening)?;
        let prefix = tag.strip_suffix('c').context("invalid cell tag")?;
        let opening = type_attribute
            .replace_all(opening.trim_end_matches('>'), "")
            .into_owned();
        replacements.push((
            cell.range(),
            format!(
                "{opening}><{prefix}f>{}</{prefix}f></{tag}>",
                xml_attr(formula)
            ),
        ));
    }
    ensure!(
        replacements.len() == edits.len(),
        "a formula cell to edit is missing on {sheet}"
    );
    splice(text, replacements, "formula")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_must_close_and_use_the_file_notation() {
        for formula in [
            "SUM(A1:A3)*2",
            "IF(A1=\"a(\",'My ''Sheet'''!B2,#N/A)",
            "_xlfn.XLOOKUP(A1,Table1[[#This Row],[Key]],C:C)",
            "SUM({1,2;3,4})",
            "[1]Other!A1+_xlfn.IFS(A1,1)",
        ] {
            check_formula(formula).unwrap_or_else(|e| panic!("{formula}: {e}"));
        }
        for (formula, message) in [
            ("", "empty"),
            ("=A1", "single leading"),
            ("SUM(A1", "( is not closed"),
            ("A1)", ") has no matching"),
            ("\"open", "string literal"),
            ("'Sheet!A1", "sheet name"),
            ("Table1[Key", "[ is not closed"),
            ("{1,2", "{ is not closed"),
            ("XLOOKUP(A1,B:B,C:C)", "_xlfn.XLOOKUP"),
            ("A1+ifs(A1,1)", "_xlfn.ifs"),
            ("_xlfn._xlws.FILTER(A:A,B:B)", "enter this formula in Excel"),
            ("_xlfn.LET(x,1,x)", "enter this formula in Excel"),
            ("SUM(A1#)", "spill reference"),
            ("@A:A", "_xlfn.SINGLE"),
        ] {
            let error = check_formula(formula).unwrap_err().to_string();
            assert!(error.contains(message), "{formula}: {error}");
        }
    }

    #[test]
    fn edited_cells_lose_their_cache_and_shared_groups_are_expanded() {
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>1</v></c><c r="B1" s="3" t="str"><f t="shared" ref="B1:C1" si="0">A1*2</f><v>2</v></c><c r="C1"><f t="shared" si="0"/><v>4</v></c><c r="D1"><f ca="1">NOW()</f><v>1</v></c></row></sheetData></worksheet>"#;
        let edits = BTreeMap::from([
            ("B1".to_owned(), "A1*3&\"<\"".to_owned()),
            ("D1".to_owned(), "TODAY()".to_owned()),
        ]);
        let edited = edit_formulas(sheet, "S", &edits).unwrap();
        assert!(
            edited.contains(r#"<c r="B1" s="3"><f>A1*3&amp;&quot;&lt;&quot;</f></c>"#),
            "{edited}"
        );
        // The member of the master's group keeps the formula it derived.
        assert!(
            edited.contains(r#"<c r="C1"><f>B1*2</f><v>4</v></c>"#),
            "{edited}"
        );
        assert!(
            edited.contains(r#"<c r="D1"><f>TODAY()</f></c>"#),
            "{edited}"
        );

        let array = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><f t="array" ref="A1:A2">B1:B2</f><v>1</v></c><c r="B1"><v>1</v></c></row></sheetData></worksheet>"#;
        for (address, message) in [("A1", "array formula"), ("B1", "holds no formula")] {
            let edits = BTreeMap::from([(address.to_owned(), "1".to_owned())]);
            let error = edit_formulas(array, "S", &edits).unwrap_err().to_string();
            assert!(error.contains(message), "{address}: {error}");
        }
    }
}
