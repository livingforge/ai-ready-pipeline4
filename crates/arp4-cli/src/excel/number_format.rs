//! The text a cell shows: its value under its number format, as Excel draws
//! it. A format this does not render (conditions, fractions, other numeral
//! systems) shows the value as stored, never a guess.
use serde_json::Value;

/// The text Excel shows for `value` under `format`. `date1904` is the
/// workbook's date system.
pub fn display(value: &Value, format: &str, date1904: bool) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(true) => "TRUE".into(),
        Value::Bool(false) => "FALSE".into(),
        Value::Number(number) => match number.as_f64() {
            Some(x) => self::number(x, format, date1904).unwrap_or_else(|| general(x)),
            None => number.to_string(),
        },
        Value::String(text) => self::text(text, format).unwrap_or_else(|| text.clone()),
        other => other.to_string(),
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Literal(String),
    /// `0`, `#` or `?`.
    Digit(char),
    Point,
    Comma,
    Percent,
    /// `E+` (true) or `E-`.
    Exponent(bool),
    At,
    General,
    /// A date or time code, lowercased: `yyyy`, `m`, `[h]`, `ampm`, `ap`, …
    Date(String),
}

/// The sections of a format, split at `;` outside quotes, escapes and brackets.
fn sections(format: &str) -> Vec<&str> {
    let mut sections = vec![];
    let (mut start, mut quoted, mut bracket, mut escaped) = (0, false, false, false);
    for (i, c) in format.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if !quoted => escaped = true,
            '"' => quoted = !quoted,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            ';' if !quoted && !bracket => {
                sections.push(&format[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    sections.push(&format[start..]);
    sections
}

struct Section {
    tokens: Vec<Token>,
    /// Japanese locale (`[$-411]`): era names, 午前/午後 and N月.
    japanese: bool,
}

/// None for a section this does not render.
fn tokens(section: &str) -> Option<Section> {
    let chars: Vec<char> = section.chars().collect();
    let mut tokens = vec![];
    let mut japanese = false;
    let mut i = 0;
    let starts = |i: usize, word: &str| {
        let word: Vec<char> = word.chars().collect();
        chars.len() >= i + word.len()
            && chars[i..i + word.len()]
                .iter()
                .zip(&word)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    };
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '"' => {
                let mut literal = String::new();
                while i < chars.len() && chars[i] != '"' {
                    literal.push(chars[i]);
                    i += 1;
                }
                i += 1;
                tokens.push(Token::Literal(literal));
            }
            '\\' => {
                if let Some(next) = chars.get(i) {
                    tokens.push(Token::Literal(next.to_string()));
                }
                i += 1;
            }
            '_' => {
                tokens.push(Token::Literal(" ".into()));
                i += 1;
            }
            '*' => i += 1,
            '[' => {
                let mut content = String::new();
                while i < chars.len() && chars[i] != ']' {
                    content.push(chars[i]);
                    i += 1;
                }
                i += 1;
                let lower = content.to_ascii_lowercase();
                if let Some(currency) = content.strip_prefix('$') {
                    let (symbol, locale) = currency.split_once('-').unwrap_or((currency, ""));
                    let locale = locale.to_ascii_lowercase();
                    japanese |= locale.ends_with("411") || locale.starts_with("ja");
                    if !symbol.is_empty() {
                        tokens.push(Token::Literal(symbol.into()));
                    }
                } else if !lower.is_empty()
                    && ['h', 'm', 's']
                        .iter()
                        .any(|code| lower.chars().all(|c| c == *code))
                {
                    tokens.push(Token::Date(format!("[{}]", &lower[..1])));
                } else if [
                    "black", "blue", "cyan", "green", "magenta", "red", "white", "yellow",
                ]
                .contains(&lower.as_str())
                    || lower.starts_with("color")
                {
                } else {
                    // Conditions, numeral systems and the like.
                    return None;
                }
            }
            '0' | '#' | '?' => tokens.push(Token::Digit(c)),
            '.' => tokens.push(Token::Point),
            ',' => tokens.push(Token::Comma),
            '%' => tokens.push(Token::Percent),
            '@' => tokens.push(Token::At),
            'E' | 'e' if matches!(chars.get(i), Some('+' | '-')) => {
                tokens.push(Token::Exponent(chars[i] == '+'));
                i += 1;
            }
            _ if starts(i - 1, "general") => {
                tokens.push(Token::General);
                i += 6;
            }
            _ if starts(i - 1, "am/pm") => {
                tokens.push(Token::Date("ampm".into()));
                i += 4;
            }
            _ if starts(i - 1, "a/p") => {
                tokens.push(Token::Date("ap".into()));
                i += 2;
            }
            _ if starts(i - 1, "aaa") => {
                let mut run = 3;
                i += 2;
                while chars.get(i).is_some_and(|c| c.eq_ignore_ascii_case(&'a')) {
                    run += 1;
                    i += 1;
                }
                tokens.push(Token::Date("a".repeat(run.min(4))));
            }
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' | 'g' | 'G' | 'e' => {
                let code = c.to_ascii_lowercase();
                let mut run = String::from(code);
                while chars.get(i).is_some_and(|c| c.to_ascii_lowercase() == code) {
                    run.push(code);
                    i += 1;
                }
                tokens.push(Token::Date(run));
            }
            'b' | 'B' if matches!(chars.get(i), Some('b' | 'B' | '1' | '2')) => return None,
            _ => tokens.push(Token::Literal(c.to_string())),
        }
    }
    Some(Section { tokens, japanese })
}

/// Whether a format code is one Excel replaces by the system's: the built-in
/// short date (numFmtId 14) and date-time (22) and the `[$-F800]` long date
/// and `[$-F400]` time. They show as Japanese Excel shows them.
fn system(format: &str) -> &str {
    match format {
        "mm-dd-yy" => "yyyy/m/d",
        "m/d/yy h:mm" => "yyyy/m/d h:mm",
        _ if format.to_ascii_uppercase().contains("[$-F800]") => r#"yyyy"年"m"月"d"日""#,
        _ if format.to_ascii_uppercase().contains("[$-F400]") => "h:mm:ss",
        _ => format,
    }
}

fn number(x: f64, format: &str, date1904: bool) -> Option<String> {
    let format = system(format);
    if format.is_empty() || format.eq_ignore_ascii_case("general") {
        return Some(general(x));
    }
    let sections = sections(format);
    // A section for negative numbers shows their magnitude, without a sign.
    let (section, value) = match sections.len() {
        1 => (sections[0], x),
        2 if x < 0.0 => (sections[1], -x),
        2 => (sections[0], x),
        _ if x < 0.0 => (sections[1], -x),
        _ if x == 0.0 => (sections[2], x),
        _ => (sections[0], x),
    };
    let section = tokens(section)?;
    if section.tokens.iter().any(|t| matches!(t, Token::Date(_))) {
        date(value, &section, date1904)
    } else {
        numeric(value, &section.tokens)
    }
}

fn text(text: &str, format: &str) -> Option<String> {
    let sections = sections(format);
    let section = match sections.len() {
        4.. => sections[3],
        1 if sections[0].contains('@') => sections[0],
        _ => return None,
    };
    let mut shown = String::new();
    for token in tokens(section)?.tokens {
        match token {
            Token::Literal(literal) => shown.push_str(&literal),
            Token::At => shown.push_str(text),
            _ => return None,
        }
    }
    Some(shown)
}

/// The decimal digits of |x| as Excel keeps them, 15 significant: the integer
/// digits without leading zeros (empty below 1) and exactly `places`
/// decimals, rounded half away from zero.
fn fixed(x: f64, places: usize) -> (String, String) {
    let scientific = format!("{:.14e}", x.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let point = exponent + 1;
    let (mut integer, mut fraction) = if point <= 0 {
        (String::new(), "0".repeat(-point as usize) + &digits)
    } else if point as usize >= digits.len() {
        (
            digits.clone() + &"0".repeat(point as usize - digits.len()),
            String::new(),
        )
    } else {
        (
            digits[..point as usize].to_owned(),
            digits[point as usize..].to_owned(),
        )
    };
    if fraction.len() > places {
        let up = fraction.as_bytes()[places] >= b'5';
        fraction.truncate(places);
        if up {
            let mut all: Vec<u8> = format!("{integer}{fraction}").into_bytes();
            let mut i = all.len();
            loop {
                if i == 0 {
                    all.insert(0, b'1');
                    break;
                }
                i -= 1;
                if all[i] == b'9' {
                    all[i] = b'0';
                } else {
                    all[i] += 1;
                    break;
                }
            }
            let all = String::from_utf8(all).unwrap();
            let split = all.len() - places;
            integer = all[..split].to_owned();
            fraction = all[split..].to_owned();
        }
    }
    while fraction.len() < places {
        fraction.push('0');
    }
    let integer = integer.trim_start_matches('0').to_owned();
    (integer, fraction)
}

/// Excel's General: up to 15 significant digits, without trailing zeros;
/// scientific beyond them.
fn general(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let sign = if x < 0.0 { "-" } else { "" };
    let abs = x.abs();
    let exponent = abs.log10().floor() as i32;
    if !(-9..15).contains(&exponent) {
        let (integer, fraction) = fixed(abs / 10f64.powi(exponent), 14);
        let fraction = fraction.trim_end_matches('0');
        let point = if fraction.is_empty() { "" } else { "." };
        let (integer, exponent) = if integer.len() > 1 {
            ("1".to_owned(), exponent + 1)
        } else {
            (integer, exponent)
        };
        let sign_e = if exponent < 0 { '-' } else { '+' };
        return format!(
            "{sign}{integer}{point}{fraction}E{sign_e}{:02}",
            exponent.abs()
        );
    }
    let places = (14 - exponent).max(0) as usize;
    let (integer, fraction) = fixed(abs, places);
    let fraction = fraction.trim_end_matches('0');
    let integer = if integer.is_empty() { "0" } else { &integer };
    if fraction.is_empty() {
        format!("{sign}{integer}")
    } else {
        format!("{sign}{integer}.{fraction}")
    }
}

fn numeric(x: f64, tokens: &[Token]) -> Option<String> {
    let negative = x < 0.0;
    let mut value = x.abs();
    let digit = |t: &Token| matches!(t, Token::Digit(_));
    let exponent_at = tokens.iter().position(|t| matches!(t, Token::Exponent(_)));
    let mantissa = &tokens[..exponent_at.unwrap_or(tokens.len())];
    let point_at = mantissa.iter().position(|t| *t == Token::Point);
    let first = mantissa.iter().position(|t| digit(t) || *t == Token::Point);
    let Some(first) = first else {
        // Only text: the sign stays, as Excel shows it.
        let mut shown = if negative {
            "-".to_owned()
        } else {
            String::new()
        };
        for token in tokens {
            match token {
                Token::Literal(literal) => shown.push_str(literal),
                Token::General | Token::At => shown.push_str(&general(value)),
                Token::Percent => shown.push('%'),
                Token::Comma => {}
                _ => return None,
            }
        }
        return Some(shown);
    };
    let integer_end = point_at.unwrap_or_else(|| {
        mantissa
            .iter()
            .rposition(digit)
            .map_or(mantissa.len(), |last| last + 1)
    });
    let last_integer_digit = mantissa[..integer_end].iter().rposition(digit);
    // A comma between integer digits groups thousands; one after them scales.
    let mut thousands = false;
    for (i, token) in mantissa.iter().enumerate() {
        if *token == Token::Comma {
            if last_integer_digit.is_some_and(|last| i > first && i < last) {
                thousands = true;
            } else if i > first {
                value /= 1000.0;
            }
        }
    }
    for token in tokens {
        if *token == Token::Percent {
            value *= 100.0;
        }
    }
    // A fraction written with a slash is not rendered.
    if tokens.iter().any(|t| *t == Token::Literal("/".into())) {
        return None;
    }
    let integer_tokens: Vec<&Token> = mantissa[first..integer_end]
        .iter()
        .filter(|t| **t != Token::Comma)
        .collect();
    let fraction_tokens: &[Token] = match point_at {
        Some(point) => {
            let end = mantissa.iter().rposition(digit).filter(|e| *e > point);
            &mantissa[point + 1..end.map_or(point + 1, |e| e + 1)]
        }
        None => &[],
    };
    let suffix_start = match point_at {
        Some(point) => point + 1 + fraction_tokens.len(),
        None => last_integer_digit.map_or(integer_end, |l| l + 1),
    };
    let places = fraction_tokens.iter().filter(|t| digit(t)).count();
    let integer_places = integer_tokens.iter().filter(|t| digit(t)).count();
    let mut exponent_text = String::new();
    let mut after_exponent = suffix_start;
    if let Some(at) = exponent_at {
        let Token::Exponent(plus) = tokens[at] else {
            unreachable!()
        };
        let width = tokens[at + 1..].iter().take_while(|t| digit(t)).count();
        after_exponent = at + 1 + width;
        let mut exponent = if value == 0.0 {
            0
        } else {
            value.log10().floor() as i32
        };
        let span = integer_places.max(1) as i32;
        let grouped = integer_tokens.iter().any(|t| **t == Token::Digit('#'));
        exponent = if grouped && span > 1 {
            exponent.div_euclid(span) * span
        } else {
            exponent - (span - 1)
        };
        let (integer, _) = fixed(value / 10f64.powi(exponent), places);
        if !grouped && integer.len() > span as usize {
            exponent += 1;
        }
        value /= 10f64.powi(exponent);
        let sign = if exponent < 0 {
            "-"
        } else if plus {
            "+"
        } else {
            ""
        };
        exponent_text = format!("E{sign}{:0width$}", exponent.abs(), width = width.max(1));
    }
    let (integer, fraction) = fixed(value, places);
    let mut shown = String::new();
    if negative {
        shown.push('-');
    }
    for token in &mantissa[..first] {
        shown.push_str(&literal(token, value)?);
    }
    // Integer digits fill the placeholders from the right; digits beyond
    // them go to the leftmost one.
    let mut digits: Vec<char> = integer.chars().collect();
    if thousands {
        let zeros = integer_tokens
            .iter()
            .filter(|t| matches!(t, Token::Digit('0')))
            .count();
        while digits.len() < zeros {
            digits.insert(0, '0');
        }
        let mut grouped = String::new();
        for (i, c) in digits.iter().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(*c);
        }
        shown.push_str(&grouped);
    } else {
        let mut filled = vec![];
        let mut remaining = integer_places;
        for token in integer_tokens.iter().rev() {
            match token {
                Token::Digit(placeholder) => {
                    remaining -= 1;
                    match digits.pop() {
                        Some(c) => {
                            filled.push(c.to_string());
                            if remaining == 0 {
                                while let Some(c) = digits.pop() {
                                    filled.push(c.to_string());
                                }
                            }
                        }
                        None => filled.push(
                            match placeholder {
                                '0' => "0",
                                '?' => " ",
                                _ => "",
                            }
                            .into(),
                        ),
                    }
                }
                other => filled.push(literal(other, value)?),
            }
        }
        // A format without integer placeholders still shows the integer.
        filled.extend(digits.into_iter().rev().map(String::from));
        filled.reverse();
        shown.push_str(&filled.concat());
    }
    if point_at.is_some() {
        shown.push('.');
        let last_nonzero = fraction.rfind(|c| c != '0').map_or(0, |i| i + 1);
        let mut position = 0;
        for token in fraction_tokens {
            match token {
                Token::Digit(placeholder) => {
                    shown.push_str(match (position < last_nonzero, placeholder) {
                        (true, _) => &fraction[position..position + 1],
                        (false, '0') => "0",
                        (false, '?') => " ",
                        _ => "",
                    });
                    position += 1;
                }
                other => shown.push_str(&literal(other, value)?),
            }
        }
    }
    let rest_start = match exponent_at {
        Some(at) => {
            for token in &mantissa[suffix_start..at] {
                shown.push_str(&literal(token, value)?);
            }
            shown.push_str(&exponent_text);
            after_exponent
        }
        None => suffix_start,
    };
    for token in &tokens[rest_start..] {
        shown.push_str(&literal(token, value)?);
    }
    Some(shown)
}

/// A token outside the digits of a number.
fn literal(token: &Token, value: f64) -> Option<String> {
    Some(match token {
        Token::Literal(literal) => literal.clone(),
        Token::Percent => "%".into(),
        Token::Comma => String::new(),
        Token::General | Token::At => general(value),
        _ => return None,
    })
}

/// Days from 1970-01-01 to a date (Howard Hinnant's days_from_civil), and back.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const JAPANESE_WEEKDAYS: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];
/// A Japanese era from its first day.
struct Era {
    start: (i64, u32, u32),
    name: &'static str,
    initial: &'static str,
    latin: &'static str,
}

const ERAS: [Era; 5] = [
    Era {
        start: (2019, 5, 1),
        name: "令和",
        initial: "令",
        latin: "R",
    },
    Era {
        start: (1989, 1, 8),
        name: "平成",
        initial: "平",
        latin: "H",
    },
    Era {
        start: (1926, 12, 25),
        name: "昭和",
        initial: "昭",
        latin: "S",
    },
    Era {
        start: (1912, 7, 30),
        name: "大正",
        initial: "大",
        latin: "T",
    },
    Era {
        start: (1868, 1, 1),
        name: "明治",
        initial: "明",
        latin: "M",
    },
];

fn date(serial: f64, section: &Section, date1904: bool) -> Option<String> {
    if serial < 0.0 {
        return None;
    }
    let tokens = &section.tokens;
    // Decimals of seconds written after `s`: `ss.00`.
    let mut subsecond = 0;
    for (i, token) in tokens.iter().enumerate() {
        if *token == Token::Point
            && tokens[..i]
                .iter()
                .rev()
                .find(|t| matches!(t, Token::Date(_)))
                .is_some_and(
                    |t| matches!(t, Token::Date(code) if code.starts_with('s') || code == "[s]"),
                )
        {
            subsecond = tokens[i + 1..]
                .iter()
                .take_while(|t| **t == Token::Digit('0'))
                .count();
        }
    }
    let unit = 10i64.pow(subsecond as u32);
    let total = (serial * 86_400.0 * unit as f64).round() as i64;
    let days = total.div_euclid(86_400 * unit);
    let within = total - days * 86_400 * unit;
    let (hour, minute, second, fraction) = (
        within / unit / 3600,
        within / unit / 60 % 60,
        within / unit % 60,
        within % unit,
    );
    let (year, month, day) = if date1904 {
        // Serial 0 is 1904-01-01.
        civil(days - 24_107)
    } else if days == 60 {
        (1900, 2, 29)
    } else if days == 0 {
        (1900, 1, 0)
    } else if days < 60 {
        civil(days - 25_568)
    } else {
        civil(days - 25_569)
    };
    let weekday = if date1904 {
        (days + 5).rem_euclid(7)
    } else {
        (days + 6).rem_euclid(7)
    } as usize;
    let twelve = tokens
        .iter()
        .any(|t| matches!(t, Token::Date(code) if code == "ampm" || code == "ap"));
    let era = ERAS.iter().find(|era| (year, month, day) >= era.start);
    let japanese_era = section.japanese
        || tokens
            .iter()
            .any(|t| matches!(t, Token::Date(code) if code.starts_with('g')));
    let codes: Vec<(usize, &str)> = tokens
        .iter()
        .enumerate()
        .filter_map(|(i, t)| match t {
            Token::Date(code) => Some((i, code.as_str())),
            _ => None,
        })
        .collect();
    let mut shown = String::new();
    let mut skip_digits = 0;
    for (i, token) in tokens.iter().enumerate() {
        if skip_digits > 0 && *token == Token::Digit('0') {
            skip_digits -= 1;
            continue;
        }
        let code = match token {
            Token::Date(code) => code.as_str(),
            Token::Point
                if subsecond > 0 && i + 1 < tokens.len() && tokens[i + 1] == Token::Digit('0') =>
            {
                shown.push('.');
                shown.push_str(&format!("{fraction:0subsecond$}"));
                skip_digits = subsecond;
                continue;
            }
            Token::Literal(literal) => {
                shown.push_str(literal);
                continue;
            }
            Token::Point => {
                shown.push('.');
                continue;
            }
            Token::Comma => {
                shown.push(',');
                continue;
            }
            Token::Percent => {
                shown.push('%');
                continue;
            }
            Token::Digit(c) => {
                shown.push(*c);
                continue;
            }
            _ => return None,
        };
        let at = codes.iter().position(|(index, _)| *index == i).unwrap();
        let minute_here = (code == "m" || code == "mm")
            && (codes[..at]
                .last()
                .is_some_and(|(_, c)| c.starts_with('h') || *c == "[h]")
                || codes
                    .get(at + 1)
                    .is_some_and(|(_, c)| c.starts_with('s') || *c == "[s]"));
        let era_year = era.map(|era| year - era.start.0 + 1);
        let piece = match code {
            "y" | "yy" => format!("{:02}", year % 100),
            _ if code.starts_with('y') => format!("{year:04}"),
            "m" if minute_here => minute.to_string(),
            "mm" if minute_here => format!("{minute:02}"),
            "m" => month.to_string(),
            "mm" => format!("{month:02}"),
            "mmm" | "mmmm" if section.japanese => format!("{month}月"),
            "mmm" => MONTHS[month as usize - 1][..3].to_owned(),
            "mmmmm" => MONTHS[month as usize - 1][..1].to_owned(),
            _ if code.starts_with('m') => MONTHS[month as usize - 1].to_owned(),
            "d" => day.to_string(),
            "dd" => format!("{day:02}"),
            "ddd" => WEEKDAYS[weekday][..3].to_owned(),
            _ if code.starts_with('d') => WEEKDAYS[weekday].to_owned(),
            "aaa" => JAPANESE_WEEKDAYS[weekday].to_owned(),
            "aaaa" => format!("{}曜日", JAPANESE_WEEKDAYS[weekday]),
            "h" | "hh" => {
                let hour = if twelve { (hour + 11) % 12 + 1 } else { hour };
                if code == "h" {
                    hour.to_string()
                } else {
                    format!("{hour:02}")
                }
            }
            "[h]" => (days * 24 + hour).to_string(),
            "[m]" => ((days * 24 + hour) * 60 + minute).to_string(),
            "[s]" => (((days * 24 + hour) * 60 + minute) * 60 + second).to_string(),
            "s" => second.to_string(),
            "ss" => format!("{second:02}"),
            "ampm" if section.japanese => if hour < 12 { "午前" } else { "午後" }.into(),
            "ampm" => if hour < 12 { "AM" } else { "PM" }.into(),
            "ap" => if hour < 12 { "A" } else { "P" }.into(),
            "g" => era?.latin.to_owned(),
            "gg" => era?.initial.to_owned(),
            _ if code.starts_with('g') => era?.name.to_owned(),
            "e" if japanese_era => era_year?.to_string(),
            "ee" if japanese_era => format!("{:02}", era_year?),
            _ if code.starts_with('e') => year.to_string(),
            _ => return None,
        };
        shown.push_str(&piece);
    }
    Some(shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shown(value: Value, format: &str) -> String {
        display(&value, format, false)
    }

    #[test]
    fn general_numbers_keep_fifteen_digits_without_float_noise() {
        assert_eq!(shown(json!(30), "General"), "30");
        assert_eq!(shown(json!(0.1 + 0.2), "General"), "0.3");
        assert_eq!(shown(json!(-1.5), ""), "-1.5");
        assert_eq!(shown(json!(1.0 / 3.0), "General"), "0.333333333333333");
        assert_eq!(shown(json!(123_456_789_012_i64), "General"), "123456789012");
        assert_eq!(shown(json!(1.5e20), "General"), "1.5E+20");
        assert_eq!(shown(json!(true), "General"), "TRUE");
        assert_eq!(shown(json!("注文"), "General"), "注文");
        assert_eq!(shown(Value::Null, "0"), "");
    }

    #[test]
    fn numbers_follow_placeholders_grouping_scaling_and_sections() {
        assert_eq!(shown(json!(1234.5), "#,##0"), "1,235");
        assert_eq!(shown(json!(1234.5), "#,##0.00"), "1,234.50");
        assert_eq!(shown(json!(2.675), "0.00"), "2.68");
        assert_eq!(shown(json!(0.5), "0%"), "50%");
        assert_eq!(shown(json!(0.125), "0.0%"), "12.5%");
        assert_eq!(shown(json!(0.5), "#.00"), ".50");
        assert_eq!(shown(json!(1), "#.##"), "1.");
        assert_eq!(shown(json!(1.5), "0.0#"), "1.5");
        assert_eq!(shown(json!(7), "000"), "007");
        assert_eq!(shown(json!(1_234_567), "#,##0,"), "1,235");
        assert_eq!(shown(json!(1_234_567), "0.0,,"), "1.2");
        assert_eq!(shown(json!(1_234_567), "000-0000"), "123-4567");
        assert_eq!(shown(json!(-5), "#,##0;[Red]-#,##0"), "-5");
        assert_eq!(shown(json!(-5), "#,##0;(#,##0)"), "(5)");
        assert_eq!(shown(json!(-5), r##""$"#,##0"##), "-$5");
        assert_eq!(shown(json!(0), r##"0;-0;"ゼロ""##), "ゼロ");
        assert_eq!(shown(json!(1500), r##"#,##0"円""##), "1,500円");
        assert_eq!(shown(json!(1500), r##"[$¥-411]#,##0"##), "¥1,500");
        assert_eq!(
            shown(
                json!(1500),
                r##"_(* #,##0_);_(* \(#,##0\);_(* "-"_);_(@_)"##
            ),
            " 1,500 "
        );
        assert_eq!(
            shown(json!(0), r##"_(* #,##0_);_(* \(#,##0\);_(* "-"_);_(@_)"##),
            " - "
        );
        assert_eq!(shown(json!(12_345), "0.00E+00"), "1.23E+04");
        assert_eq!(shown(json!(0.000_12), "0.0E+0"), "1.2E-4");
        assert_eq!(shown(json!(12_345), "##0.0E+0"), "12.3E+3");
        assert_eq!(shown(json!(42), r##""No."0"##), "No.42");
    }

    #[test]
    fn text_takes_the_text_section() {
        assert_eq!(shown(json!("abc"), r##"0;-0;0;"「"@"」""##), "「abc」");
        assert_eq!(shown(json!("abc"), "@"), "abc");
        assert_eq!(shown(json!("abc"), "0.00"), "abc");
    }

    #[test]
    fn dates_and_times_show_as_the_workbook_does() {
        // 2026-10-10 is serial 46305 in the 1900 system.
        let day = 46_305.0;
        assert_eq!(shown(json!(day), "mm-dd-yy"), "2026/10/10");
        assert_eq!(shown(json!(day), "yyyy/mm/dd"), "2026/10/10");
        assert_eq!(
            shown(json!(day), r##"yyyy"年"m"月"d"日""##),
            "2026年10月10日"
        );
        assert_eq!(shown(json!(day), "d-mmm-yy"), "10-Oct-26");
        assert_eq!(shown(json!(day), "dddd, mmmm d"), "Saturday, October 10");
        assert_eq!(shown(json!(day), "m/d(aaa)"), "10/10(土)");
        assert_eq!(
            shown(json!(day), r##"[$-411]ggge"年"m"月"d"日""##),
            "令和8年10月10日"
        );
        assert_eq!(shown(json!(day), "[$-411]ge.m.d"), "R8.10.10");
        assert_eq!(shown(json!(32_516), r##"[$-411]ggge"年""##), "平成1年");
        assert_eq!(shown(json!(32_515), r##"[$-411]ggge"年""##), "昭和64年");
        assert_eq!(shown(json!(day + 0.5), "m/d/yy h:mm"), "2026/10/10 12:00");
        assert_eq!(shown(json!(0.75), "h:mm AM/PM"), "6:00 PM");
        assert_eq!(shown(json!(0.75), "[$-411]AM/PM h:mm"), "午後 6:00");
        assert_eq!(shown(json!(1.5), "[h]:mm"), "36:00");
        assert_eq!(shown(json!(0.000_7), "mm:ss.0"), "01:00.5");
        assert_eq!(
            shown(json!(day), "[$-F800]dddd, mmmm dd, yyyy"),
            "2026年10月10日"
        );
        assert_eq!(shown(json!(60), "yyyy/m/d"), "1900/2/29");
        assert_eq!(shown(json!(61), "yyyy/m/d"), "1900/3/1");
        assert_eq!(
            display(&json!(day - 1462.0), "yyyy/m/d", true),
            "2026/10/10"
        );
    }

    #[test]
    fn unrendered_formats_show_the_stored_value() {
        assert_eq!(shown(json!(1234), "[<=999]000;0000-00"), "1234");
        assert_eq!(shown(json!(0.5), "# ?/?"), "0.5");
        assert_eq!(shown(json!(123), "[DBNum1]General"), "123");
        assert_eq!(shown(json!(-1), "yyyy/m/d"), "-1");
    }
}
