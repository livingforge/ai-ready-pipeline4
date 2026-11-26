//! What a sheet lays over the formats of its cells: the fills, text and
//! borders the styles of its tables give, and those its conditional formats
//! give the cells that meet their rules, with their data bars, colour scales
//! and icons. A rule whose formula is not worked out here gives nothing.
use super::*;
use crate::excel::{
    BorderSide, ConditionalLook, DifferentialLook, Rule, SheetLook, TableLook, Test, Threshold,
};
use foldhash::HashMap;

/// What is laid over a cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Laid {
    pub fill: Option<String>,
    pub color: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub strike: Option<bool>,
    pub underline: Option<bool>,
    /// The left, right, top and bottom sides of the border.
    pub borders: [Option<BorderSide>; 4],
    /// A data bar: the share of the cell's width it fills, and its colour.
    pub bar: Option<(f64, String)>,
    /// An icon before the value: its character and colour.
    pub icon: Option<(&'static str, &'static str)>,
    /// The rule shows its bar or icon without the value.
    pub hide_value: bool,
    /// The fill and the text are the style of a table's, which the cell's
    /// own format goes over; a conditional format goes over the cell's.
    pub under: bool,
}

impl Laid {
    /// Takes what `format` gives and this has not yet.
    fn take(&mut self, format: &DifferentialLook) {
        let keep = |own: &mut Option<String>, given: &Option<String>| {
            if own.is_none() {
                own.clone_from(given);
            }
        };
        keep(&mut self.fill, &format.fill);
        keep(&mut self.color, &format.color);
        self.bold = self.bold.or(format.bold);
        self.italic = self.italic.or(format.italic);
        self.strike = self.strike.or(format.strike);
        self.underline = self.underline.or(format.underline);
    }
}

/// The value of a cell as a formula takes it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Val {
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error,
}

impl Val {
    pub(super) fn of(value: &Value, error: bool) -> Self {
        match value {
            Value::Null => Self::Empty,
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => n.as_f64().map_or(Self::Empty, Self::Number),
            Value::String(_) if error => Self::Error,
            Value::String(text) => Self::Text(text.clone()),
            _ => Self::Empty,
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Empty => Some(0.0),
            Self::Bool(b) => Some(f64::from(u8::from(*b))),
            Self::Text(text) => text.trim().parse().ok(),
            Self::Error => None,
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Empty | Self::Error => String::new(),
            Self::Number(n) => {
                crate::excel::number_format::display(&serde_json::json!(n), "General", false)
            }
            Self::Text(text) => text.clone(),
            Self::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).to_owned(),
        }
    }

    fn truth(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Number(n) => Some(*n != 0.0),
            Self::Empty => Some(false),
            _ => None,
        }
    }

    /// How this stands to `other` as Excel orders values: numbers before
    /// text before TRUE and FALSE, text without regard to case, an empty
    /// cell as the zero or the empty text it is compared with.
    fn compare(&self, other: &Self) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        let rank = |v: &Self| match v {
            Self::Number(_) => 0,
            Self::Text(_) => 1,
            Self::Bool(_) => 2,
            _ => 3,
        };
        match (self, other) {
            (Self::Error, _) | (_, Self::Error) => None,
            (Self::Empty, Self::Empty) => Some(Ordering::Equal),
            (Self::Empty, Self::Text(_)) => Self::Text(String::new()).compare(other),
            (Self::Text(_), Self::Empty) => self.compare(&Self::Text(String::new())),
            (Self::Empty, _) => Self::Number(0.0).compare(other),
            (_, Self::Empty) => self.compare(&Self::Number(0.0)),
            (Self::Number(a), Self::Number(b)) => a.partial_cmp(b),
            (Self::Text(a), Self::Text(b)) => Some(a.to_lowercase().cmp(&b.to_lowercase())),
            (Self::Bool(a), Self::Bool(b)) => Some(a.cmp(b)),
            (a, b) => Some(rank(a).cmp(&rank(b))),
        }
    }
}

/// What is laid over each cell of a sheet, by (column, row). `value` gives
/// the value a cell shows.
pub(super) fn laid(look: &SheetLook, value: &dyn Fn(u32, u32) -> Val) -> HashMap<(u32, u32), Laid> {
    let mut cells: HashMap<(u32, u32), Laid> = HashMap::default();
    // Conditional formats first: what an earlier rule gives stays.
    let mut stopped: foldhash::HashSet<(u32, u32)> = foldhash::HashSet::default();
    for conditional in &look.conditional {
        apply(conditional, value, &mut cells, &mut stopped);
    }
    for table in &look.tables {
        style(table, &mut cells);
    }
    cells
}

/// Lays the style of `table` under what its cells have.
fn style(table: &TableLook, cells: &mut HashMap<(u32, u32), Laid>) {
    let (c0, r0, c1, r1) = table.range;
    // A table larger than a sheet a person reads is not styled cell by cell.
    if u64::from(c1 - c0 + 1) * u64::from(r1 - r0 + 1) > 200_000 {
        return;
    }
    let body = (r0 + table.header_rows, r1.saturating_sub(table.totals_rows));
    for r in r0..=r1 {
        for c in c0..=c1 {
            let header = r < body.0;
            let total = r > body.1;
            let mut laid = Laid::default();
            if header {
                laid.take(&table.header);
            } else if total {
                laid.take(&table.total);
            }
            if !header && !total {
                if table.first_column && c == c0 {
                    laid.take(&table.first);
                }
                if table.last_column && c == c1 {
                    laid.take(&table.last);
                }
                if table.row_stripes {
                    laid.take(&table.row_stripe[((r - body.0) % 2) as usize]);
                }
                if table.column_stripes {
                    laid.take(&table.column_stripe[((c - c0) % 2) as usize]);
                }
            }
            laid.take(&table.whole);
            // The edges of the table, then the lines between its rows and
            // columns; the header and the totals draw their own edges.
            let whole = &table.whole;
            laid.borders = [
                (c == c0).then(|| whole.borders[0].clone()).flatten(),
                if c == c1 {
                    whole.borders[1].clone()
                } else {
                    whole.inner[1].clone()
                },
                (r == r0).then(|| whole.borders[2].clone()).flatten(),
                if r == r1 {
                    whole.borders[3].clone()
                } else {
                    whole.inner[0].clone()
                },
            ];
            if header && r + 1 == body.0 && table.header.borders[3].is_some() {
                laid.borders[3].clone_from(&table.header.borders[3]);
            }
            if total && r == body.1 + 1 && table.total.borders[2].is_some() {
                laid.borders[2].clone_from(&table.total.borders[2]);
            }
            let cell = cells.entry((c, r)).or_default();
            // A conditional format already there stays over the style.
            let under = cell == &Laid::default();
            if cell.fill.is_none() {
                cell.fill = laid.fill;
            }
            if cell.color.is_none() {
                cell.color = laid.color;
            }
            cell.bold = cell.bold.or(laid.bold);
            cell.italic = cell.italic.or(laid.italic);
            cell.strike = cell.strike.or(laid.strike);
            cell.underline = cell.underline.or(laid.underline);
            for (own, given) in cell.borders.iter_mut().zip(laid.borders) {
                if own.is_none() {
                    *own = given;
                }
            }
            cell.under = under;
        }
    }
}

/// Lays what `conditional` gives over the cells of its ranges meeting it.
fn apply(
    conditional: &ConditionalLook,
    value: &dyn Fn(u32, u32) -> Val,
    cells: &mut HashMap<(u32, u32), Laid>,
    stopped: &mut foldhash::HashSet<(u32, u32)>,
) {
    let places: Vec<(u32, u32)> = conditional
        .ranges
        .iter()
        .flat_map(|&(c0, r0, c1, r1)| {
            // A rule over whole columns or rows is looked at where a sheet
            // a person reads has cells.
            let (c1, r1) = (c1.min(c0 + 255), r1.min(r0 + 4_999));
            (r0..=r1).flat_map(move |r| (c0..=c1).map(move |c| (c, r)))
        })
        .collect();
    let values: Vec<Val> = places.iter().map(|&(c, r)| value(c, r)).collect();
    let mut numbers: Vec<f64> = values
        .iter()
        .filter_map(|v| match v {
            Val::Number(n) => Some(*n),
            _ => None,
        })
        .collect();
    numbers.sort_by(f64::total_cmp);
    let resolve = |threshold: &Threshold| -> Option<f64> {
        let (min, max) = (*numbers.first()?, *numbers.last()?);
        Some(match threshold {
            Threshold::Min => min,
            Threshold::Max => max,
            Threshold::Number(n) => *n,
            Threshold::Percent(p) => min + (max - min) * p / 100.0,
            Threshold::Percentile(p) => {
                let at = (numbers.len() - 1) as f64 * p / 100.0;
                let (low, high) = (at.floor() as usize, at.ceil() as usize);
                numbers[low] + (numbers[high] - numbers[low]) * (at - low as f64)
            }
            Threshold::Formula(text) => text.trim().parse().ok()?,
        })
    };
    let origin = conditional.ranges[0];
    match &conditional.rule {
        Rule::ColorScale(stops) => {
            let Some(stops) = stops
                .iter()
                .map(|(threshold, color)| Some((resolve(threshold)?, color.as_str())))
                .collect::<Option<Vec<(f64, &str)>>>()
            else {
                return;
            };
            for (place, value) in places.iter().zip(&values) {
                let Val::Number(number) = value else {
                    continue;
                };
                if stopped.contains(place) {
                    continue;
                }
                let cell = cells.entry(*place).or_default();
                if cell.fill.is_none() {
                    cell.fill = Some(scaled(&stops, *number));
                }
            }
        }
        Rule::DataBar {
            min,
            max,
            color,
            show_value,
        } => {
            let (Some(min), Some(max)) = (resolve(min), resolve(max)) else {
                return;
            };
            for (place, value) in places.iter().zip(&values) {
                let Val::Number(number) = value else {
                    continue;
                };
                if stopped.contains(place) {
                    continue;
                }
                let share = if max > min {
                    ((number - min) / (max - min)).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let cell = cells.entry(*place).or_default();
                if cell.bar.is_none() {
                    cell.bar = Some((share, color.clone()));
                    cell.hide_value |= !show_value;
                }
            }
        }
        Rule::IconSet {
            set,
            thresholds,
            reverse,
            show_value,
        } => {
            let Some(bounds) = thresholds.iter().map(resolve).collect::<Option<Vec<f64>>>() else {
                return;
            };
            for (place, value) in places.iter().zip(&values) {
                let Val::Number(number) = value else {
                    continue;
                };
                if stopped.contains(place) || bounds.is_empty() {
                    continue;
                }
                // The icon of the highest threshold the value reaches.
                let mut level = bounds.iter().rposition(|b| number >= b).unwrap_or(0);
                if *reverse {
                    level = bounds.len() - 1 - level;
                }
                let cell = cells.entry(*place).or_default();
                if cell.icon.is_none() {
                    cell.icon = Some(icon(set, level, bounds.len()));
                    cell.hide_value |= !show_value;
                }
            }
        }
        Rule::Format { test, format, stop } => {
            let average =
                (!numbers.is_empty()).then(|| numbers.iter().sum::<f64>() / numbers.len() as f64);
            let mut counts: HashMap<String, usize> = HashMap::default();
            if matches!(test, Test::Duplicates(_)) {
                for value in &values {
                    if *value != Val::Empty {
                        *counts.entry(value.text().to_lowercase()).or_default() += 1;
                    }
                }
            }
            for (place, own) in places.iter().zip(&values) {
                if stopped.contains(place) {
                    continue;
                }
                let shift = (
                    i64::from(place.0) - i64::from(origin.0),
                    i64::from(place.1) - i64::from(origin.1),
                );
                let formula = |text: &str| evaluate(text, shift, *place, value);
                let met = match test {
                    Test::CellIs { operator, operands } => {
                        let operands: Option<Vec<Val>> =
                            operands.iter().map(|o| formula(o)).collect();
                        operands.and_then(|operands| cell_is(own, operator, &operands))
                    }
                    Test::Expression(text) => formula(text).and_then(|v| v.truth()),
                    Test::Text { kind, text } => {
                        let (own, text) = (own.text().to_lowercase(), text.to_lowercase());
                        Some(match kind.as_str() {
                            "containsText" => own.contains(&text),
                            "notContainsText" => !own.contains(&text),
                            "beginsWith" => own.starts_with(&text),
                            _ => own.ends_with(&text),
                        })
                    }
                    Test::Blanks(blank) => Some((own.text().trim().is_empty()) == *blank),
                    Test::Errors(error) => Some((*own == Val::Error) == *error),
                    Test::Top {
                        rank,
                        percent,
                        bottom,
                    } => match own {
                        Val::Number(number) if !numbers.is_empty() => {
                            let count = if *percent {
                                ((numbers.len() as f64 * rank / 100.0).floor() as usize).max(1)
                            } else {
                                *rank as usize
                            }
                            .clamp(1, numbers.len());
                            Some(if *bottom {
                                *number <= numbers[count - 1]
                            } else {
                                *number >= numbers[numbers.len() - count]
                            })
                        }
                        _ => Some(false),
                    },
                    Test::Average { above, equal } => match (own, average) {
                        (Val::Number(number), Some(average)) => Some(
                            (*equal && *number == average)
                                || if *above {
                                    *number > average
                                } else {
                                    *number < average
                                },
                        ),
                        _ => Some(false),
                    },
                    Test::Duplicates(duplicate) => Some(
                        *own != Val::Empty
                            && (counts.get(&own.text().to_lowercase()).copied().unwrap_or(0) > 1)
                                == *duplicate,
                    ),
                    Test::Other => None,
                };
                if met == Some(true) {
                    let cell = cells.entry(*place).or_default();
                    cell.take(format);
                    for (own, given) in cell.borders.iter_mut().zip(&format.borders) {
                        if own.is_none() {
                            own.clone_from(given);
                        }
                    }
                    if *stop {
                        stopped.insert(*place);
                    }
                }
            }
        }
    }
}

/// Whether `own` stands to the operands as `operator` says.
fn cell_is(own: &Val, operator: &str, operands: &[Val]) -> Option<bool> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let first = own.compare(operands.first()?)?;
    Some(match operator {
        "lessThan" => first == Less,
        "lessThanOrEqual" => first != Greater,
        "equal" => first == Equal,
        "notEqual" => first != Equal,
        "greaterThanOrEqual" => first != Less,
        "greaterThan" => first == Greater,
        "between" | "notBetween" => {
            let second = own.compare(operands.get(1)?)?;
            // Between the two, whichever is the lower.
            let within =
                (first != Less && second != Greater) || (first != Greater && second != Less);
            within == (operator == "between")
        }
        _ => return None,
    })
}

/// The colour a colour scale gives `number` between its stops, `RRGGBB`.
fn scaled(stops: &[(f64, &str)], number: f64) -> String {
    let part = |rgb: &str, at: usize| {
        rgb.get(at..at + 2)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            .map_or(0.0, f64::from)
    };
    let (first, last) = (stops[0], stops[stops.len() - 1]);
    if number <= first.0 {
        return first.1.to_owned();
    }
    if number >= last.0 {
        return last.1.to_owned();
    }
    for pair in stops.windows(2) {
        let ((low, from), (high, to)) = (pair[0], pair[1]);
        if number <= high && high > low {
            let share = (number - low) / (high - low);
            return (0..3)
                .map(|k| {
                    let value = part(from, k * 2) * (1.0 - share) + part(to, k * 2) * share;
                    format!("{:02X}", value.round().clamp(0.0, 255.0) as u8)
                })
                .collect();
        }
    }
    last.1.to_owned()
}

/// The icon of an icon set for the `level`th of `count` steps, from the
/// lowest: its character and colour.
fn icon(set: &str, level: usize, count: usize) -> (&'static str, &'static str) {
    const RED: &str = "#d65532";
    const YELLOW: &str = "#e9b83f";
    const GREEN: &str = "#4f9d83";
    const GREY: &str = "#7f7f7f";
    // Where the step stands from the lowest (0) to the highest (4).
    let step = if count <= 1 {
        4
    } else {
        level.min(count - 1) * 4 / (count - 1)
    };
    let grey = set.contains("Gray");
    let color = match step {
        _ if grey => GREY,
        0 => RED,
        1..=3 if count == 3 => YELLOW,
        1 | 2 => YELLOW,
        3 if count >= 5 => YELLOW,
        _ => GREEN,
    };
    let shape = if set.contains("Arrows") {
        ["↓", "↘", "→", "↗", "↑"][step]
    } else if set.contains("Symbols") {
        ["✖", "!", "!", "!", "✔"][step]
    } else if set.contains("Flags") {
        "⚑"
    } else if set.contains("Signs") {
        ["◆", "▲", "▲", "▲", "●"][step]
    } else if set.contains("Rating") || set.contains("Quarters") || set.contains("Boxes") {
        ["○", "◔", "◑", "◕", "●"][step]
    } else {
        "●"
    };
    (shape, color)
}

/// A token of a formula.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Text(String),
    /// A name: a function, TRUE or FALSE, or a reference to a cell.
    Name(String),
    Operator(&'static str),
    Open,
    Close,
    Comma,
}

fn tokens(text: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '"' => {
                let mut literal = String::new();
                i += 1;
                loop {
                    match (chars.get(i), chars.get(i + 1)) {
                        (Some('"'), Some('"')) => {
                            literal.push('"');
                            i += 2;
                        }
                        (Some('"'), _) => {
                            i += 1;
                            break;
                        }
                        (Some(c), _) => {
                            literal.push(*c);
                            i += 1;
                        }
                        (None, _) => return None,
                    }
                }
                tokens.push(Token::Text(literal));
            }
            '0'..='9' | '.' => {
                let start = i;
                while chars
                    .get(i)
                    .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                {
                    i += 1;
                }
                let number: String = chars[start..i].iter().collect();
                tokens.push(Token::Number(number.parse().ok()?));
            }
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '<' | '>' | '=' | '+' | '-' | '*' | '/' | '&' => {
                let pair: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                let operator = match (pair.as_str(), c) {
                    ("<=", _) => "<=",
                    (">=", _) => ">=",
                    ("<>", _) => "<>",
                    (_, '<') => "<",
                    (_, '>') => ">",
                    (_, '=') => "=",
                    (_, '+') => "+",
                    (_, '-') => "-",
                    (_, '*') => "*",
                    (_, '/') => "/",
                    _ => "&",
                };
                i += operator.len();
                tokens.push(Token::Operator(operator));
            }
            c if c.is_alphabetic() || c == '$' || c == '_' => {
                let start = i;
                while chars
                    .get(i)
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '$' | '_' | '.'))
                {
                    i += 1;
                }
                tokens.push(Token::Name(chars[start..i].iter().collect()));
            }
            // A range, another sheet's cell and the like.
            _ => return None,
        }
    }
    Some(tokens)
}

/// The value of the formula `text`, written for the first cell of a range,
/// at the cell `shift` columns and rows from it, which is at `place`; none
/// for a formula with what is not worked out here (ranges, other sheets,
/// names, most functions).
fn evaluate(
    text: &str,
    shift: (i64, i64),
    place: (u32, u32),
    value: &dyn Fn(u32, u32) -> Val,
) -> Option<Val> {
    let tokens = tokens(text.trim().trim_start_matches('='))?;
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
        shift,
        place,
        value,
        depth: 0,
    };
    let result = parser.comparison()?;
    (parser.at == tokens.len()).then_some(result)
}

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
    shift: (i64, i64),
    place: (u32, u32),
    value: &'a dyn Fn(u32, u32) -> Val,
    depth: usize,
}

impl Parser<'_> {
    fn operator(&mut self, among: &[&'static str]) -> Option<&'static str> {
        match self.tokens.get(self.at) {
            Some(Token::Operator(operator)) if among.contains(operator) => {
                self.at += 1;
                Some(operator)
            }
            _ => None,
        }
    }

    fn comparison(&mut self) -> Option<Val> {
        use std::cmp::Ordering::{Equal, Greater, Less};
        self.depth += 1;
        if self.depth > 64 {
            return None;
        }
        let mut left = self.joined()?;
        while let Some(operator) = self.operator(&["<=", ">=", "<>", "<", ">", "="]) {
            let right = self.joined()?;
            let order = left.compare(&right)?;
            left = Val::Bool(match operator {
                "<=" => order != Greater,
                ">=" => order != Less,
                "<>" => order != Equal,
                "<" => order == Less,
                ">" => order == Greater,
                _ => order == Equal,
            });
        }
        self.depth -= 1;
        Some(left)
    }

    fn joined(&mut self) -> Option<Val> {
        let mut left = self.sum()?;
        while self.operator(&["&"]).is_some() {
            let right = self.sum()?;
            left = Val::Text(left.text() + &right.text());
        }
        Some(left)
    }

    fn sum(&mut self) -> Option<Val> {
        let mut left = self.product()?;
        while let Some(operator) = self.operator(&["+", "-"]) {
            let (a, b) = (left.number()?, self.product()?.number()?);
            left = Val::Number(if operator == "+" { a + b } else { a - b });
        }
        Some(left)
    }

    fn product(&mut self) -> Option<Val> {
        let mut left = self.signed()?;
        while let Some(operator) = self.operator(&["*", "/"]) {
            let (a, b) = (left.number()?, self.signed()?.number()?);
            left = if operator == "*" {
                Val::Number(a * b)
            } else if b == 0.0 {
                Val::Error
            } else {
                Val::Number(a / b)
            };
        }
        Some(left)
    }

    fn signed(&mut self) -> Option<Val> {
        match self.operator(&["-", "+"]) {
            Some("-") => Some(Val::Number(-self.signed()?.number()?)),
            Some(_) => self.signed(),
            None => self.primary(),
        }
    }

    fn primary(&mut self) -> Option<Val> {
        let token = self.tokens.get(self.at)?.clone();
        self.at += 1;
        match token {
            Token::Number(number) => Some(Val::Number(number)),
            Token::Text(text) => Some(Val::Text(text)),
            Token::Open => {
                let inner = self.comparison()?;
                (self.tokens.get(self.at) == Some(&Token::Close)).then(|| {
                    self.at += 1;
                    inner
                })
            }
            Token::Name(name) if self.tokens.get(self.at) == Some(&Token::Open) => {
                self.at += 1;
                let mut arguments = vec![];
                if self.tokens.get(self.at) == Some(&Token::Close) {
                    self.at += 1;
                } else {
                    loop {
                        arguments.push(self.comparison()?);
                        match self.tokens.get(self.at) {
                            Some(Token::Comma) => self.at += 1,
                            Some(Token::Close) => {
                                self.at += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                self.function(&name.to_ascii_uppercase(), &arguments)
            }
            Token::Name(name) => match name.to_ascii_uppercase().as_str() {
                "TRUE" => Some(Val::Bool(true)),
                "FALSE" => Some(Val::Bool(false)),
                _ => self.cell(&name),
            },
            _ => None,
        }
    }

    /// The value of the cell a reference names: a column or row without `$`
    /// moves as the cell the formula is worked out for does.
    fn cell(&self, reference: &str) -> Option<Val> {
        let (column_fixed, rest) = match reference.strip_prefix('$') {
            Some(rest) => (true, rest),
            None => (false, reference),
        };
        let letters = rest.chars().take_while(char::is_ascii_alphabetic).count();
        let (letters, rest) = rest.split_at(letters);
        let (row_fixed, digits) = match rest.strip_prefix('$') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        if letters.is_empty() || letters.len() > 3 || digits.is_empty() {
            return None;
        }
        let (column, row) =
            crate::excel::coordinate(&format!("{}{digits}", letters.to_ascii_uppercase())).ok()?;
        let moved = |at: u32, fixed: bool, shift: i64| {
            let at = i64::from(at) + if fixed { 0 } else { shift };
            u32::try_from(at).ok().filter(|at| *at >= 1)
        };
        Some((self.value)(
            moved(column, column_fixed, self.shift.0)?,
            moved(row, row_fixed, self.shift.1)?,
        ))
    }

    fn function(&self, name: &str, arguments: &[Val]) -> Option<Val> {
        let one = || (arguments.len() == 1).then(|| &arguments[0]);
        Some(match name {
            "AND" => Val::Bool(
                arguments
                    .iter()
                    .map(Val::truth)
                    .collect::<Option<Vec<bool>>>()?
                    .into_iter()
                    .all(|b| b),
            ),
            "OR" => Val::Bool(
                arguments
                    .iter()
                    .map(Val::truth)
                    .collect::<Option<Vec<bool>>>()?
                    .into_iter()
                    .any(|b| b),
            ),
            "NOT" => Val::Bool(!one()?.truth()?),
            "ISBLANK" => Val::Bool(*one()? == Val::Empty),
            "ISNUMBER" => Val::Bool(matches!(one()?, Val::Number(_))),
            "ISTEXT" => Val::Bool(matches!(one()?, Val::Text(_))),
            "ISERROR" | "ISERR" => Val::Bool(*one()? == Val::Error),
            "LEN" => Val::Number(one()?.text().chars().count() as f64),
            "TRIM" => Val::Text(one()?.text().trim().to_owned()),
            "UPPER" => Val::Text(one()?.text().to_uppercase()),
            "LOWER" => Val::Text(one()?.text().to_lowercase()),
            "ABS" => Val::Number(one()?.number()?.abs()),
            "INT" => Val::Number(one()?.number()?.floor()),
            "LEFT" | "RIGHT" => {
                let text: Vec<char> = arguments.first()?.text().chars().collect();
                let count = match arguments.get(1) {
                    Some(count) => count.number()?.max(0.0) as usize,
                    None => 1,
                }
                .min(text.len());
                Val::Text(if name == "LEFT" {
                    text[..count].iter().collect()
                } else {
                    text[text.len() - count..].iter().collect()
                })
            }
            "MOD" => {
                let (a, b) = (arguments.first()?.number()?, arguments.get(1)?.number()?);
                if arguments.len() != 2 || b == 0.0 {
                    return None;
                }
                Val::Number(a - b * (a / b).floor())
            }
            "ROW" if arguments.is_empty() => Val::Number(f64::from(self.place.1)),
            "COLUMN" if arguments.is_empty() => Val::Number(f64::from(self.place.0)),
            "IF" => {
                let met = arguments.first()?.truth()?;
                match (met, arguments.get(1), arguments.get(2)) {
                    (true, Some(then), _) => then.clone(),
                    (false, _, Some(otherwise)) => otherwise.clone(),
                    (false, _, None) => Val::Bool(false),
                    _ => return None,
                }
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(cells: &[((u32, u32), Val)]) -> impl Fn(u32, u32) -> Val + '_ {
        move |c, r| {
            cells
                .iter()
                .find(|(place, _)| *place == (c, r))
                .map_or(Val::Empty, |(_, value)| value.clone())
        }
    }

    #[test]
    fn formulas_are_worked_out_for_the_cell_they_are_laid_over() {
        let cells = [
            ((1, 1), Val::Text("済".into())),
            ((2, 1), Val::Number(12.0)),
            ((1, 2), Val::Text("未".into())),
            ((2, 2), Val::Number(3.0)),
        ];
        let value = values(&cells);
        let at = |text: &str, row: u32| evaluate(text, (0, i64::from(row) - 1), (2, row), &value);
        assert_eq!(at(r#"$A1="済""#, 1), Some(Val::Bool(true)));
        assert_eq!(at(r#"$A1="済""#, 2), Some(Val::Bool(false)));
        assert_eq!(at("AND($B1>=10,LEN($A1)>0)", 1), Some(Val::Bool(true)));
        assert_eq!(at("AND($B1>=10,LEN($A1)>0)", 2), Some(Val::Bool(false)));
        assert_eq!(at("MOD(ROW(),2)=0", 2), Some(Val::Bool(true)));
        assert_eq!(at("$B$1+B1*2", 2), Some(Val::Number(18.0)));
        assert_eq!(at(r#"A1&"-"&B1"#, 2), Some(Val::Text("未-3".into())));
        // An empty cell is zero to a number and nothing to a text.
        assert_eq!(at(r#"OR(C1=0,C1="")"#, 1), Some(Val::Bool(true)));
        // What is not worked out gives nothing, rather than a guess.
        assert_eq!(at("SUM(A1:B1)>0", 1), None);
        assert_eq!(at("Sheet2!A1=1", 1), None);
        assert_eq!(at("COUNTIF($A1,1)", 1), None);
    }

    #[test]
    fn rules_give_their_formats_bars_colours_and_icons() {
        let cells: Vec<((u32, u32), Val)> = (1..=5)
            .map(|c| ((c, 1), Val::Number(f64::from(c) * 10.0)))
            .collect();
        let value = values(&cells);
        let format = DifferentialLook {
            fill: Some("FFFF00".into()),
            bold: Some(true),
            ..DifferentialLook::default()
        };
        let range = vec![(1, 1, 5, 1)];
        let rule = |priority: i64, rule: Rule| ConditionalLook {
            ranges: range.clone(),
            priority,
            rule,
        };
        let mut laid: HashMap<(u32, u32), Laid> = HashMap::default();
        let mut stopped = foldhash::HashSet::default();
        for conditional in [
            rule(
                1,
                Rule::Format {
                    test: Test::CellIs {
                        operator: "greaterThan".into(),
                        operands: vec!["30".into()],
                    },
                    format: Box::new(format.clone()),
                    stop: true,
                },
            ),
            rule(
                2,
                Rule::ColorScale(vec![
                    (Threshold::Min, "FF0000".into()),
                    (Threshold::Max, "0000FF".into()),
                ]),
            ),
            rule(
                3,
                Rule::DataBar {
                    min: Threshold::Min,
                    max: Threshold::Max,
                    color: "638EC6".into(),
                    show_value: true,
                },
            ),
            rule(
                4,
                Rule::IconSet {
                    set: "3TrafficLights1".into(),
                    thresholds: vec![
                        Threshold::Percent(0.0),
                        Threshold::Percent(33.0),
                        Threshold::Percent(67.0),
                    ],
                    reverse: false,
                    show_value: true,
                },
            ),
        ] {
            apply(&conditional, &value, &mut laid, &mut stopped);
        }
        // Over 30: the format, and nothing of the rules after it.
        assert_eq!(laid[&(4, 1)].fill.as_deref(), Some("FFFF00"));
        assert_eq!(laid[&(4, 1)].bold, Some(true));
        assert_eq!(
            (laid[&(4, 1)].bar.clone(), laid[&(4, 1)].icon),
            (None, None)
        );
        // The others: a colour between the ends, a bar and an icon.
        assert_eq!(laid[&(1, 1)].fill.as_deref(), Some("FF0000"));
        assert_eq!(laid[&(3, 1)].fill.as_deref(), Some("800080"));
        assert_eq!(laid[&(1, 1)].bar, Some((0.0, "638EC6".into())));
        assert_eq!(laid[&(3, 1)].bar, Some((0.5, "638EC6".into())));
        assert_eq!(laid[&(1, 1)].icon.unwrap().0, "●");
        assert_ne!(laid[&(1, 1)].icon.unwrap().1, laid[&(3, 1)].icon.unwrap().1);
    }

    #[test]
    fn a_table_style_goes_under_its_cells_with_its_stripes_and_edges() {
        let thin = |color: &str| {
            Some(BorderSide {
                style: "thin".into(),
                color: Some(color.into()),
            })
        };
        let table = TableLook {
            range: (2, 2, 3, 5),
            header_rows: 1,
            totals_rows: 0,
            row_stripes: true,
            whole: DifferentialLook {
                borders: [
                    thin("111111"),
                    thin("111111"),
                    thin("111111"),
                    thin("111111"),
                ],
                inner: [thin("222222"), None],
                ..DifferentialLook::default()
            },
            header: DifferentialLook {
                fill: Some("4472C4".into()),
                color: Some("FFFFFF".into()),
                bold: Some(true),
                ..DifferentialLook::default()
            },
            row_stripe: [
                DifferentialLook {
                    fill: Some("D9E1F2".into()),
                    ..DifferentialLook::default()
                },
                DifferentialLook::default(),
            ],
            ..TableLook::default()
        };
        let mut laid = HashMap::default();
        style(&table, &mut laid);
        assert_eq!(laid[&(2, 2)].fill.as_deref(), Some("4472C4"));
        assert_eq!(laid[&(2, 2)].color.as_deref(), Some("FFFFFF"));
        assert!(laid[&(2, 2)].under);
        assert_eq!(laid[&(2, 3)].fill.as_deref(), Some("D9E1F2"));
        assert_eq!(laid[&(2, 4)].fill, None);
        assert_eq!(laid[&(2, 5)].fill.as_deref(), Some("D9E1F2"));
        // The edge on the left of the first column only, the lines between
        // the rows under all but the last.
        assert_eq!(laid[&(2, 3)].borders[0], thin("111111"));
        assert_eq!(laid[&(3, 3)].borders[0], None);
        assert_eq!(laid[&(3, 3)].borders[3], thin("222222"));
        assert_eq!(laid[&(3, 5)].borders[3], thin("111111"));
    }
}
