//! A tiny `key=value` style language that maps one-to-one onto GPUI's
//! Tailwind-shaped [`gpui::Styled`] helpers.
//!
//! A style is a whitespace-separated list of tokens:
//!
//! ```text
//! "flex-row gap=8 p=16 bg=#1e1e2e rounded=10 shadow=lg"
//! ```
//!
//! Every token either names a GPUI helper directly (`row`, `center`, `bold`,
//! `truncate`) or carries `= value` for one that takes an argument. Unknown
//! tokens are collected by [`unknown_tokens`] so a typo is visible rather
//! than silent.


use gpui::{hsla, px, relative, rgb, rgba, FontWeight, Hsla, StyleRefinement, Styled};

/// Applies every token in `spec` to `el`, in order.
pub fn apply<T: Styled>(mut el: T, spec: &str) -> T {
    for (key, value) in tokens(spec) {
        el = one(el, &key, value.as_deref());
    }
    el
}

/// Applies `spec` to a plain [`StyleRefinement`].
///
/// `StyleRefinement` is itself `Styled`, so the same token vocabulary works
/// for hover/active/focus refinements as for whole elements.
pub fn refine(mut r: StyleRefinement, spec: &str) -> StyleRefinement {
    for (key, value) in tokens(spec) {
        r = one(r, &key, value.as_deref());
    }
    r
}

/// Splits a style string into lower-cased `(key, Option<value>)` pairs.
pub fn tokens(spec: &str) -> Vec<(String, Option<String>)> {
    spec.split_whitespace()
        .map(|raw| match raw.split_once('=') {
            Some((k, v)) => (k.to_ascii_lowercase(), Some(v.to_string())),
            None => (raw.to_ascii_lowercase(), None),
        })
        .collect()
}

/// The tokens in `spec` this module does not understand.
pub fn unknown_tokens(spec: &str) -> Vec<String> {
    tokens(spec)
        .into_iter()
        .filter(|(k, _)| !is_known(k))
        .map(|(k, v)| match v {
            Some(v) => format!("{k}={v}"),
            None => k,
        })
        .collect()
}

fn is_known(key: &str) -> bool {
    matches!(
        key,
        // display and flex containers
        "row" | "col" | "wrap" | "nowrap" | "grid" | "block" | "hidden" | "visible"
        | "absolute" | "relative"
        // flex item behaviour
        | "grow" | "shrink" | "flex" 
        // spacing
        | "gap" | "gapx" | "gapy"
        | "p" | "px" | "py" | "pt" | "pb" | "pl" | "pr"
        | "m" | "mx" | "my" | "mt" | "mb" | "ml" | "mr"
        | "auto"
        // size
        | "w" | "h" | "size" | "minw" | "maxw" | "minh" | "maxh" | "basis"
        // position
        | "inset" | "top" | "right" | "bottom" | "left"
        // alignment
        | "justify" | "items" | "content" | "align"
        // paint
        | "bg" | "grad" | "color" | "opacity"
        // border and decoration
        | "border" | "bordercolor" | "dashed" | "rounded" | "shadow"
        // type
        | "font" | "fs" | "weight" | "bold" | "semibold" | "italic" | "underline" | "strike"
        | "lh" | "truncate"
        // behaviour
        | "cursor" | "overflow" | "debug"
        // grid placement and decoration
        | "gridcols" | "gridrows" | "colspan" | "rowspan" | "slash"
        | "wavy" | "deco" | "decocolor" | "rotate"
        | "bordert" | "borderb" | "borderl" | "borderr" | "overflowx"
        | "ellipsis" | "clamp" | "ws" | "textoverflow"
        | "roundedtl" | "roundedtr" | "roundedbl" | "roundedbr"
        | "focusable" | "tabstop" | "tabindex"
        | "w_full" | "wfull" | "h_full" | "hfull" | "size_full" | "sizefull"
        // interaction refinements (handled by `view`, listed here so they are
        // not reported as unknown)
        | "hover" | "active"
    )
}

/// The interaction tokens in `spec` (`hover=...`, `active=...`), if any.
pub fn interaction(spec: &str) -> (Option<String>, Option<String>) {
    let mut hover = None;
    let mut active = None;
    for (key, value) in tokens(spec) {
        match (key.as_str(), value) {
            ("hover", Some(v)) => hover = Some(decode_quoted(&v)),
            ("active", Some(v)) => active = Some(decode_quoted(&v)),
            _ => {}
        }
    }
    (hover, active)
}

/// `Some("x")`, `Some("y")`, or `None`, for the `overflow=` token.
///
/// Scrolling is a `StatefulInteractiveElement` capability, so the element has
/// to be given an id before `view` can turn it on.
pub fn scroll_axis(spec: &str) -> Option<&'static str> {
    for (key, value) in tokens(spec) {
        if key == "overflow" {
            match value.as_deref() {
                Some("x") => return Some("x"),
                Some("y") => return Some("y"),
                // `overflow_scroll()` — a box that scrolls either way.
                Some("scroll") | Some("both") => return Some("both"),
                _ => {}
            }
        }
    }
    None
}

/// `rotate=<degrees>`, for the elements that can carry a transformation.
pub fn rotation(spec: &str) -> Option<f32> {
    tokens(spec)
        .into_iter()
        .find(|(key, _)| key == "rotate")
        .and_then(|(_, value)| value)
        .and_then(|value| value.parse::<f32>().ok())
}

/// `shadow=#rrggbbaa,x,y,blur,spread`, with `|` between several shadows.
fn shadows(v: &str) -> Vec<gpui::BoxShadow> {
    v.split('|')
        .map(|one| {
            let parts: Vec<&str> = one.split(',').collect();
            let at = |i: usize, fallback: f32| {
                parts.get(i).and_then(|p| p.parse::<f32>().ok()).unwrap_or(fallback)
            };
            gpui::BoxShadow {
                color: colour(parts.first().copied().unwrap_or("#00000080")),
                offset: gpui::point(px(at(1, 0.0)), px(at(2, 0.0))),
                blur_radius: px(at(3, 0.0)),
                spread_radius: px(at(4, 0.0)),
            }
        })
        .collect()
}

/// `slash=#rrggbb,width,interval`.
fn slash(v: &str) -> gpui::Background {
    let parts: Vec<&str> = v.split(',').collect();
    let colour = colour(parts.first().copied().unwrap_or("#ffffff"));
    let width = parts.get(1).and_then(|w| w.parse::<f32>().ok()).unwrap_or(4.0);
    let interval = parts.get(2).and_then(|i| i.parse::<f32>().ok()).unwrap_or(width);
    gpui::pattern_slash(colour, width, interval)
}

/// `hover=bg:#313244 opacity:0.9` is awkward to nest inside a whitespace
/// separated token, so nested refinements use `_` for the separating space:
/// `hover=bg:#313244_opacity:0.9`. A leading `bg:#x` is shorthand for `bg=#x`.
fn decode_quoted(raw: &str) -> String {
    let mut out = String::new();
    for (i, part) in raw.split('_').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if let Some((k, v)) = part.split_once(':') {
            out.push_str(k);
            out.push('=');
            out.push_str(v);
        } else {
            out.push_str(part);
        }
    }
    out
}

fn one<T: Styled>(el: T, key: &str, value: Option<&str>) -> T {
    let v = value.unwrap_or("");
    match key {
        // ---- display and container direction -------------------------
        // `flex` is the display mode; `col`/`row` set it too, so both
        // `gpui::column("gap=8", …)` and a bare `"flex col"` work.
        "row" => el.flex().flex_row(),
        "col" => el.flex().flex_col(),
        "wrap" => el.flex().flex_wrap(),
        "nowrap" => el.flex().flex_nowrap(),
        "grid" => el.grid(),
        "block" => el.block(),
        "hidden" => el.invisible(),
        "visible" => el.visible(),
        "absolute" => el.absolute(),
        "relative" => el.relative(),

        // ---- flex item -------------------------------------------------
        "grow" => el.flex_grow(),
        "shrink" if v == "0" => el.flex_shrink_0(),
        "shrink" => el.flex_shrink(),
        "flex" => match v {
            "" => el.flex(),
            "1" => el.flex_1(),
            "auto" => el.flex_auto(),
            "initial" => el.flex_initial(),
            "none" => el.flex_none(),
            _ => el.flex_1(),
        },

        // ---- gap and spacing -------------------------------------------
        "gap" => el.gap(px(num(v))),
        "gapx" => el.gap_x(px(num(v))),
        "gapy" => el.gap_y(px(num(v))),
        "p" => el.p(px(num(v))),
        "px" => el.px(px(num(v))),
        "py" => el.py(px(num(v))),
        "pt" => el.pt(px(num(v))),
        "pb" => el.pb(px(num(v))),
        "pl" => el.pl(px(num(v))),
        "pr" => el.pr(px(num(v))),
        "m" => el.m(px(num(v))),
        "mx" => el.mx(px(num(v))),
        "my" => el.my(px(num(v))),
        "mt" => el.mt(px(num(v))),
        "mb" => el.mb(px(num(v))),
        "ml" => el.ml(px(num(v))),
        "mr" => el.mr(px(num(v))),
        "auto" => el.mx_auto(),

        // Bare full-size spellings, so `w_full` reads the way GPUI's own
        // helper is named.
        "w_full" | "wfull" => el.w_full(),
        "h_full" | "hfull" => el.h_full(),
        "size_full" | "sizefull" => el.size_full(),

        // ---- sizing ------------------------------------------------------
        "w" => apply_length(el, v, Axis::Width),
        "h" => apply_length(el, v, Axis::Height),
        "size" => apply_length(el, v, Axis::Both),
        "minw" => el.min_w(sized(v)),
        "maxw" => el.max_w(sized(v)),
        "minh" => el.min_h(sized(v)),
        "maxh" => el.max_h(sized(v)),
        "basis" => el.flex_basis(sized(v)),

        // ---- positioning -------------------------------------------------
        "inset" => el.inset(sized(v)),
        "top" => el.top(sized(v)),
        "right" => el.right(sized(v)),
        "bottom" => el.bottom(sized(v)),
        "left" => el.left(sized(v)),

        // ---- alignment ----------------------------------------------------
        // ---- grid placement ----------------------------------------------
        "gridcols" => el.grid_cols(num(v) as u16),
        "gridrows" => el.grid_rows(num(v) as u16),
        "colspan" => {
            if v == "full" {
                el.col_span_full()
            } else {
                el.col_span(num(v) as u16)
            }
        }
        "rowspan" => {
            if v == "full" {
                el.row_span_full()
            } else {
                el.row_span(num(v) as u16)
            }
        }

        "justify" => match v {
            "start" => el.justify_start(),
            "end" => el.justify_end(),
            "between" => el.justify_between(),
            "around" | "evenly" => el.justify_around(),
            _ => el.justify_center(),
        },
        "items" => match v {
            "start" => el.items_start(),
            "end" => el.items_end(),
            "baseline" => el.items_baseline(),
            _ => el.items_center(),
        },
        "content" => match v {
            "start" => el.content_start(),
            "end" => el.content_end(),
            "between" => el.content_between(),
            "around" => el.content_around(),
            "evenly" => el.content_evenly(),
            "stretch" => el.content_stretch(),
            _ => el.content_center(),
        },
        "align" => match v {
            "left" => el.text_left(),
            "right" => el.text_right(),
            _ => el.text_center(),
        },

        // ---- paint ---------------------------------------------------------
        "bg" => el.bg(colour(v)),
        "grad" => el.bg(gradient(v)),
        // `slash=#rrggbb,width,interval` — GPUI's repeating diagonal pattern.
        "slash" => el.bg(slash(v)),
        "color" => el.text_color(colour(v)),
        "opacity" => el.opacity(num(v) as f32),

        // ---- border and decoration ------------------------------------------
        "border" => el.border(px(num(v))),
        "bordert" => el.border_t(px(num(v))),
        "borderb" => el.border_b(px(num(v))),
        "borderl" => el.border_l(px(num(v))),
        "borderr" => el.border_r(px(num(v))),
        "bordercolor" => el.border_color(colour(v)),
        "dashed" => el.border_dashed(),
        "roundedtl" => el.rounded_tl(px(num(v))),
        "roundedtr" => el.rounded_tr(px(num(v))),
        "roundedbl" => el.rounded_bl(px(num(v))),
        "roundedbr" => el.rounded_br(px(num(v))),
        "rounded" => match v {
            "full" => el.rounded_full(),
            "xs" => el.rounded_xs(),
            "2xl" => el.rounded_2xl(),
            "3xl" => el.rounded_3xl(),
            "sm" => el.rounded_sm(),
            "md" => el.rounded_md(),
            "lg" => el.rounded_lg(),
            "xl" => el.rounded_xl(),
            "none" => el,
            _ => el.rounded(px(num(v))),
        },
        // `shadow=lg` is one of GPUI's presets; `shadow=#rrggbbaa,x,y,blur,spread`
        // is a custom `BoxShadow`, and `|` stacks several.
        "shadow" if v.contains(',') => el.shadow(shadows(v)),
        "shadow" => match v {
            "2xs" => el.shadow_2xs(),
            "xs" => el.shadow_xs(),
            "sm" => el.shadow_sm(),
            "md" => el.shadow_md(),
            "lg" => el.shadow_lg(),
            "xl" => el.shadow_xl(),
            "2xl" => el.shadow_2xl(),
            "none" => el.shadow_none(),
            _ => el.shadow_md(),
        },

        // ---- type -------------------------------------------------------------
        "font" => match v {
            "xs" => el.text_xs(),
            "sm" => el.text_sm(),
            "base" => el.text_base(),
            "lg" => el.text_lg(),
            "xl" => el.text_xl(),
            "2xl" => el.text_2xl(),
            "3xl" => el.text_3xl(),
            _ => el.text_base(),
        },
        "fs" => el.text_size(px(num(v))),
        "weight" => el.font_weight(weight(v)),
        "bold" => el.font_weight(FontWeight::BOLD),
        "semibold" => el.font_weight(FontWeight::SEMIBOLD),
        "italic" => el.italic(),
        "underline" => el.underline(),
        "strike" => el.line_through(),
        "wavy" => el.text_decoration_wavy(),
        "deco" => match v {
            "1" => el.text_decoration_1(),
            "2" => el.text_decoration_2(),
            "4" => el.text_decoration_4(),
            "8" => el.text_decoration_8(),
            _ => el,
        },
        "decocolor" => el.text_decoration_color(colour(v)),
        "lh" => el.line_height(relative(num(v) as f32)),
        "truncate" => el.truncate(),
        "ellipsis" => el.text_ellipsis(),
        // `textoverflow=<suffix>` is GPUI's `TextOverflow::Truncate(suffix)`;
        // an empty value truncates with no marker, which `ellipsis` cannot say.
        "textoverflow" => el.text_overflow(gpui::TextOverflow::Truncate(v.to_string().into())),
        "clamp" => el.line_clamp(num(v) as usize),
        "ws" => match v {
            "nowrap" => el.whitespace_nowrap(),
            _ => el.whitespace_normal(),
        },

        // ---- behaviour ----------------------------------------------------------
        "cursor" => match v {
            "pointer" => el.cursor_pointer(),
            "text" => el.cursor_text(),
            "move" => el.cursor_move(),
            "grab" => el.cursor_grab(),
            "grabbing" => el.cursor_grabbing(),
            "cross" | "crosshair" => el.cursor_crosshair(),
            "col-resize" => el.cursor_col_resize(),
            "row-resize" => el.cursor_row_resize(),
            "ew-resize" => el.cursor_ew_resize(),
            "ns-resize" => el.cursor_ns_resize(),
            "not-allowed" => el.cursor_not_allowed(),
            // `cursor_none` takes the style it suppresses; the cursor is
            // simply left unset instead.
            "none" => el,
            "open-hand" => el.cursor_grab(),
            _ => el.cursor_default(),
        },
        "overflowx" => match v {
            "hidden" => el.overflow_x_hidden(),
            "y" => el.overflow_y_hidden(),
            _ => el,
        },
        "overflow" => match v {
            "hidden" => el.overflow_hidden(),
            // Scrolling needs an element id, so `view` applies it.
            "x" | "y" | "scroll" => el,
            _ => el,
        },
        "debug" => el.debug(),

        _ => el,
    }
}

enum Axis {
    Width,
    Height,
    Both,
}

fn apply_length<T: Styled>(el: T, v: &str, axis: Axis) -> T {
    let full = v == "full" || v == "1";
    let auto = v == "auto";
    match (axis, full, auto) {
        (Axis::Width, true, _) => el.w_full(),
        (Axis::Height, true, _) => el.h_full(),
        (Axis::Both, true, _) => el.size_full(),
        (Axis::Width, false, true) => el.w_auto(),
        (Axis::Height, false, true) => el.h_auto(),
        (Axis::Both, false, true) => el,
        (Axis::Width, false, false) => el.w(sized(v)),
        (Axis::Height, false, false) => el.h(sized(v)),
        (Axis::Both, false, false) => el.size(sized(v)),
    }
}

/// `"120"` is pixels, `"50%"` is a fraction of the parent.
fn sized(v: &str) -> gpui::Length {
    if let Some(pct) = v.strip_suffix('%') {
        relative(num(pct) as f32 / 100.0).into()
    } else {
        px(num(v)).into()
    }
}

fn num(v: &str) -> f32 {
    v.parse::<f32>().unwrap_or(0.0)
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, or one of GPUI's named colours.
pub fn colour(v: &str) -> Hsla {
    if let Some(hex) = v.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let n = u32::from_str_radix(hex, 16).unwrap_or(0);
                let (r, g, b) = ((n >> 8) & 0xF, (n >> 4) & 0xF, n & 0xF);
                rgb(((r * 17) << 16) | ((g * 17) << 8) | (b * 17)).into()
            }
            6 => rgb(u32::from_str_radix(hex, 16).unwrap_or(0)).into(),
            8 => rgba(u32::from_str_radix(hex, 16).unwrap_or(0)).into(),
            _ => rgb(0x000000).into(),
        };
    }
    match v {
        "white" => gpui::white(),
        "black" => gpui::black(),
        "red" => gpui::red(),
        "green" => gpui::green(),
        "blue" => gpui::blue(),
        "yellow" => gpui::yellow(),
        "orange" => hsla(30.0 / 360.0, 1.0, 0.5, 1.0),
        "cyan" => hsla(180.0 / 360.0, 0.7, 0.6, 1.0),
        "magenta" | "pink" => hsla(320.0 / 360.0, 0.8, 0.7, 1.0),
        "purple" => hsla(270.0 / 360.0, 0.7, 0.7, 1.0),
        "transparent" => rgba(0x00000000).into(),
        _ => rgb(0x000000).into(),
    }
}

/// A background from either a bare colour (`#rrggbbaa`, a name) or a full
/// `grad=…` token. Used where a path is painted with a colour the element's
/// own style did not carry.
pub fn background(spec: &str) -> gpui::Background {
    match spec.strip_prefix("grad=") {
        Some(gradient_spec) => gradient(gradient_spec),
        None => colour(spec).into(),
    }
}

/// `grad=angle,from,to`, `grad=from,to`, or the full
/// `grad=angle,from,fromPct,to,toPct[,space]`.
///
/// `space` is `srgb` or `oklab`, matching GPUI's `ColorSpace`.
fn gradient(v: &str) -> gpui::Background {
    let parts: Vec<&str> = v.split(',').collect();
    let pct = |raw: Option<&&str>, fallback: f32| {
        raw.and_then(|p| p.parse::<f32>().ok()).unwrap_or(fallback)
    };
    let angle = |raw: &str| raw.parse::<f32>().unwrap_or(0.0);

    let (angle, from, from_pct, to, to_pct, space) = match parts.len() {
        2 => (180.0, parts[0], 0.0, parts[1], 1.0, None),
        3 => (angle(parts[0]), parts[1], 0.0, parts[2], 1.0, None),
        4 => (angle(parts[0]), parts[1], 0.0, parts[2], 1.0, Some(parts[3])),
        5 => (
            angle(parts[0]),
            parts[1],
            pct(parts.get(2), 0.0),
            parts[3],
            pct(parts.get(4), 1.0),
            None,
        ),
        _ => (
            angle(parts.first().copied().unwrap_or("0")),
            parts.get(1).copied().unwrap_or("#000000"),
            pct(parts.get(2), 0.0),
            parts.get(3).copied().unwrap_or("#ffffff"),
            pct(parts.get(4), 1.0),
            parts.get(5).copied(),
        ),
    };

    let background = gpui::linear_gradient(
        angle,
        gpui::linear_color_stop(colour(from), from_pct),
        gpui::linear_color_stop(colour(to), to_pct),
    );
    match space {
        Some("oklab") | Some("oklch") => background.color_space(gpui::ColorSpace::Oklab),
        Some("srgb") => background.color_space(gpui::ColorSpace::Srgb),
        _ => background,
    }
}

fn weight(v: &str) -> FontWeight {
    match v {
        "100" | "thin" => FontWeight::THIN,
        "200" | "extralight" => FontWeight::EXTRA_LIGHT,
        "300" | "light" => FontWeight::LIGHT,
        "500" | "medium" => FontWeight::MEDIUM,
        "600" | "semibold" => FontWeight::SEMIBOLD,
        "700" | "bold" => FontWeight::BOLD,
        "800" | "extrabold" => FontWeight::EXTRA_BOLD,
        "900" | "black" => FontWeight::BLACK,
        _ => FontWeight::NORMAL,
    }
}
