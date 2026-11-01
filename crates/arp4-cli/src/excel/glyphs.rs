//! The widths Excel measures the characters of cells in: those Windows GDI
//! gives the fonts workbooks use, in whole pixels at each size, as
//! `build/measure_glyph_widths.ps1` writes them to `glyph_widths.txt`. They
//! are hinted, so no ratio of a font's design widths gives them (Calibri's
//! digits are 7 pixels at 15, not 8).
//!
//! Excel lays a sheet out in the pixels of the screen, and fits a number to
//! its cell in the widths of 96 pixels per inch, whatever the screen
//! (measured in Excel on a screen of 120, Windows at 125%, which the view of
//! a sheet shows).
use super::fit::wide;
use std::sync::OnceLock;

/// The pixels per inch of the screen the view of a sheet lays its columns
/// out for.
pub const SCREEN: f64 = 120.0;
/// The pixels per inch Excel fits numbers to their cells in, which are the
/// view's.
pub const PAGE: f64 = 96.0;

/// The size in pixels at 96 per inch of a font of `points` drawn on the
/// screen, where GDI makes it a whole number of the screen's pixels.
pub fn screen_pixels(points: f64) -> f64 {
    (points * SCREEN / 72.0).round().max(1.0) * PAGE / SCREEN
}

const TABLE: &str = include_str!("glyph_widths.txt");

struct Table {
    chars: Vec<char>,
    first: u32,
    last: u32,
    /// The names of each font, its weight and its widths: one per character
    /// at each size from the first to the last.
    fonts: Vec<(Vec<&'static str>, u32, &'static [u8])>,
}

fn table() -> &'static Table {
    static TABLE_READ: OnceLock<Table> = OnceLock::new();
    TABLE_READ.get_or_init(|| {
        let mut table = Table {
            chars: vec![],
            first: 0,
            last: 0,
            fonts: vec![],
        };
        for line in TABLE.lines().filter(|l| !l.starts_with('#')) {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields[..] {
                ["chars", chars] => table.chars = chars.chars().collect(),
                ["sizes", first, last] => {
                    table.first = first.parse().expect("first size");
                    table.last = last.parse().expect("last size");
                }
                [names, weight, widths] => table.fonts.push((
                    names.split('|').collect(),
                    weight.parse().expect("weight"),
                    widths.as_bytes(),
                )),
                _ => panic!("glyph_widths.txt: {line}"),
            }
        }
        table
    })
}

/// A font at a size as GDI draws it.
pub struct Font {
    /// The widths at the nearest size measured, and the scale to this one.
    widths: &'static [u8],
    scale: f64,
    /// The size of an em, in pixels.
    pixels: f64,
}

impl Font {
    /// The font `name` (Calibri, Excel's default, when it is none or not
    /// measured), bold or not, of `points` at `dpi` pixels per inch.
    pub fn new(name: Option<&str>, bold: bool, points: f64, dpi: f64) -> Self {
        let table = table();
        let weight = if bold { 700 } else { 400 };
        let font = |name: &str| {
            table
                .fonts
                .iter()
                .find(|(names, w, _)| *w == weight && names.contains(&name))
        };
        let (_, _, widths) = name
            .and_then(font)
            .or_else(|| font("Calibri"))
            .expect("Calibri is measured");
        // GDI makes a font of a whole number of pixels.
        let pixels = (points * dpi / 72.0).round().max(1.0);
        let size = (pixels as u32).clamp(table.first, table.last);
        let row = table.chars.len();
        let start = (size - table.first) as usize * row;
        Self {
            widths: &widths[start..start + row],
            scale: pixels / f64::from(size),
            pixels,
        }
    }

    /// The width of `c`, in pixels: one not measured is a wide (East Asian)
    /// character an em wide, or another a digit wide.
    pub fn width(&self, c: char) -> f64 {
        match table().chars.iter().position(|&m| m == c) {
            Some(i) => f64::from(self.widths[i] - b'0') * self.scale,
            None if wide(c) => self.pixels,
            None => self.digit(),
        }
    }

    /// The width of a digit, in pixels.
    pub fn digit(&self) -> f64 {
        self.width('0')
    }

    pub fn text(&self, text: &str) -> f64 {
        text.chars().map(|c| self.width(c)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_are_those_gdi_gives_each_font_at_its_size() {
        // Calibri's digits are hinted to 7 pixels at 11 points.
        let calibri = Font::new(Some("Calibri"), false, 11.0, PAGE);
        assert_eq!(calibri.digit(), 7.0);
        assert_eq!(calibri.text("-1.5"), 5.0 + 7.0 + 4.0 + 7.0);
        assert_eq!(Font::new(Some("Calibri"), false, 11.0, SCREEN).digit(), 9.0);
        // A font is found by any of its names.
        let gothic = Font::new(Some("Yu Gothic"), false, 11.0, PAGE);
        assert_eq!(gothic.digit(), 8.0);
        assert_eq!(gothic.width('+'), 11.0);
        assert_eq!(
            Font::new(Some("游ゴシック"), false, 11.0, PAGE).width('+'),
            11.0
        );
        assert_eq!(gothic.width('円'), 15.0);
        // A font not measured is drawn as Calibri.
        assert_eq!(Font::new(Some("Unknown"), false, 11.0, PAGE).digit(), 7.0);
        assert_eq!(Font::new(None, true, 11.0, PAGE).digit(), 7.0);
        // Sizes beyond those measured scale the nearest.
        assert_eq!(
            Font::new(Some("ＭＳ ゴシック"), false, 60.0, PAGE).digit(),
            40.0
        );
    }
}
