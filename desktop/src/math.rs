//! LaTeX formulas in notes: `$…$` inside a sentence and `$$…$$` on a line of
//! its own, typeset by RaTeX into monochrome SVG. The SVG is served through the
//! app's asset source and drawn as a tinted mask, so formulas follow the theme
//! and stay sharp at every text scale.
use gpui::{prelude::*, *};
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

/// A run of note text: prose, or one formula and whether it stands alone.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Math { latex: &'a str, display: bool },
}

/// Splits text at `$$…$$` and `$…$`. `\$` is a literal dollar, and an inline
/// formula must hug its delimiters and not be followed by a digit, so prices
/// such as "$5 and $10" stay prose. Unclosed delimiters are prose too.
pub fn pieces(text: &str) -> Vec<Piece<'_>> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut prose = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        let display = bytes.get(i + 1) == Some(&b'$');
        let open = i + if display { 2 } else { 1 };
        let Some(close) = closing(text, open, display) else {
            i += 1;
            continue;
        };
        let latex = &text[open..close];
        let inline_ok = display
            || (!latex.starts_with(char::is_whitespace)
                && !latex.ends_with(char::is_whitespace)
                && !text[close + 1..].starts_with(|c: char| c.is_ascii_digit()));
        if latex.trim().is_empty() || !inline_ok {
            i = open;
            continue;
        }
        if prose < i {
            pieces.push(Piece::Text(&text[prose..i]));
        }
        pieces.push(Piece::Math {
            latex: latex.trim(),
            display,
        });
        i = close + if display { 2 } else { 1 };
        prose = i;
    }
    if prose < text.len() {
        pieces.push(Piece::Text(&text[prose..]));
    }
    pieces
}

/// The byte offset of the delimiter closing a formula that starts at `from`.
fn closing(text: &str, from: usize, display: bool) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'$' if !display => return Some(i),
            b'$' if bytes.get(i + 1) == Some(&b'$') => return Some(i),
            _ => i += 1,
        }
    }
    None
}

pub fn has_math(text: &str) -> bool {
    text.contains('$')
        && pieces(text)
            .iter()
            .any(|piece| matches!(piece, Piece::Math { .. }))
}

/// A typeset formula: its asset path and its box in em, with the baseline
/// `height` below the top.
#[derive(Clone, Debug)]
pub struct Formula {
    path: SharedString,
    width: f32,
    height: f32,
    depth: f32,
}

const ASSET_PREFIX: &str = "math/";

struct Cache {
    formulas: HashMap<(String, bool), Option<Formula>>,
    svgs: HashMap<SharedString, Arc<[u8]>>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache {
        formulas: HashMap::new(),
        svgs: HashMap::new(),
    })
});

/// The typeset formula, or `None` when RaTeX cannot parse it; the note then
/// shows the source. Results are cached; `prepare` warms the cache off the UI thread.
pub fn formula(latex: &str, display: bool) -> Option<Formula> {
    let key = (latex.to_owned(), display);
    if let Some(found) = CACHE.lock().ok()?.formulas.get(&key) {
        return found.clone();
    }
    let typeset = typeset(latex, display);
    let mut cache = CACHE.lock().ok()?;
    let formula = typeset.map(|(svg, width, height, depth)| {
        let path = SharedString::from(format!("{ASSET_PREFIX}{}.svg", cache.svgs.len()));
        cache.svgs.insert(path.clone(), svg.into_bytes().into());
        Formula {
            path,
            width,
            height,
            depth,
        }
    });
    cache.formulas.insert(key, formula.clone());
    formula
}

/// Typesets every formula in `texts`, so opening a note does not do it while drawing.
pub fn prepare<'a>(texts: impl IntoIterator<Item = &'a str>) {
    for text in texts {
        for piece in pieces(text) {
            if let Piece::Math { latex, display } = piece {
                formula(latex, display);
            }
        }
    }
}

fn typeset(latex: &str, display: bool) -> Option<(String, f32, f32, f32)> {
    use ratex_layout::{LayoutOptions, layout, to_display_list};
    use ratex_types::{color::Color, math_style::MathStyle};
    let ast = ratex_parser::parser::parse(latex).ok()?;
    let style = if display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let laid_out = layout(
        &ast,
        &LayoutOptions::default()
            .with_style(style)
            .with_color(Color::BLACK),
    );
    let svg = ratex_svg::render_to_svg(
        &to_display_list(&laid_out),
        &ratex_svg::SvgOptions {
            font_size: 32.,
            padding: 0.,
            embed_glyphs: true,
            ..Default::default()
        },
    );
    Some((
        svg,
        laid_out.width as f32,
        laid_out.height as f32,
        laid_out.depth as f32,
    ))
}

/// The SVG behind a formula's asset path.
pub fn asset(path: &str) -> Option<Cow<'static, [u8]>> {
    if !path.starts_with(ASSET_PREFIX) {
        return None;
    }
    let cache = CACHE.lock().ok()?;
    cache.svgs.get(path).map(|svg| Cow::Owned(svg.to_vec()))
}

impl Formula {
    /// The formula at `em` text size, tinted with the surrounding text color.
    /// Inline formulas sit on the text baseline: a line's content box centers
    /// near 0.35 em above the baseline, the formula's own center at
    /// (height − depth) / 2.
    pub fn element(&self, em: Rems, display: bool) -> AnyElement {
        let svg = svg()
            .path(self.path.clone())
            .flex_shrink_0()
            .w(em * self.width)
            .h(em * (self.height + self.depth));
        if display {
            svg.into_any_element()
        } else {
            svg.relative()
                .top(em * (0.35 - (self.height - self.depth) / 2.))
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Piece, pieces};

    #[test]
    fn splits_inline_and_display_formulas_and_keeps_prices_as_prose() {
        assert_eq!(
            pieces("面积 $S=\\pi r^2$，即$$\\int_a^b f(x)\\,dx$$。"),
            vec![
                Piece::Text("面积 "),
                Piece::Math {
                    latex: "S=\\pi r^2",
                    display: false
                },
                Piece::Text("，即"),
                Piece::Math {
                    latex: "\\int_a^b f(x)\\,dx",
                    display: true
                },
                Piece::Text("。"),
            ]
        );
        assert_eq!(
            pieces("costs $5 and $10"),
            vec![Piece::Text("costs $5 and $10")]
        );
        assert_eq!(pieces("\\$ 不是公式"), vec![Piece::Text("\\$ 不是公式")]);
        assert_eq!(pieces("未闭合 $x"), vec![Piece::Text("未闭合 $x")]);
    }
}
