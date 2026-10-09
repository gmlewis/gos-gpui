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


use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{hsla, px, relative, rgb, rgba, FontWeight, Hsla, StyleRefinement, Styled};

use crate::window;

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
        .filter(|(k, v)| !is_known(k) || unknown_theme_value(v) || unknown_number_value(k, v))
        .map(|(k, v)| match v {
            Some(v) => format!("{k}={v}"),
            None => k,
        })
        .collect()
}

/// Whether a token's value should have been a number and is not.
///
/// `gutter=32` is a width, and `gutter=wide` is a typo: the key is known, so the check above cannot
/// see it, and a gutter that silently became zero is exactly the silent failure the report exists to
/// prevent. Only the keys this module reads *as* numbers are checked — every other token's value is
/// a colour, a word from a closed list, or free text that `one` is entitled to reject by name.
fn unknown_number_value(key: &str, value: &Option<String>) -> bool {
    if key != "gutter" {
        return false;
    }
    match value.as_deref() {
        Some(v) => !v.is_empty() && v.parse::<f32>().is_err(),
        // A bare `gutter` is "on, at the default width" — a value is not required.
        None => false,
    }
}

/// Whether a token's value names a colour the table does not have.
///
/// `theme:<name>` is a *value*, so the key check above cannot see it. Without this,
/// `bg=theme:nosuch` would draw black in silence — the one failure mode the token
/// language exists to avoid, and the reason `unknown_tokens` reports at all.
fn unknown_theme_value(value: &Option<String>) -> bool {
    match value.as_deref().and_then(|v| v.strip_prefix("theme:")) {
        Some(name) => !is_theme_name(name),
        None => false,
    }
}

/// The names a `theme:` value may carry: the eight fields of the platform's colour
/// table, which is exactly what `gpui::colour_names` publishes.
pub fn is_theme_name(name: &str) -> bool {
    matches!(
        name,
        "text"
            | "selected_text"
            | "background"
            | "disabled"
            | "selected"
            | "border"
            | "separator"
            | "container"
    )
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
        | "lh" | "lhpx" | "truncate"
        // behaviour
        | "cursor" | "overflow" | "debug"
        // grid placement and decoration
        | "gridcols" | "gridrows" | "colspan" | "rowspan" | "slash"
        | "wavy" | "deco" | "decocolor" | "rotate"
        | "bordert" | "borderb" | "borderl" | "borderr" | "overflowx"
        | "ellipsis" | "clamp" | "ws" | "textoverflow"
        | "roundedtl" | "roundedtr" | "roundedbl" | "roundedbr"
        | "focusable" | "tabstop" | "tabindex"
        | "id"
        | "w_full" | "wfull" | "h_full" | "hfull" | "size_full" | "sizefull"
        // interaction refinements (handled by `view`, listed here so they are
        // not reported as unknown)
        | "hover" | "active" | "focus"
        // the group this element is a member of, for `gpui::group_hovered`
        | "group"
        // the keymap context an element puts itself on the stack of
        | "ctx"
        // the scrollbar thumb's colour
        | "thumb"
        // the element's own box, reported back to the program
        | "bounds"
        // the editor's own chrome: what it paints that is not a box
        | "caret" | "sel" | "gutter" | "gutterbg" | "gutterfg" | "softwrap"
    ) || is_theme_field(key.strip_prefix("theme.").unwrap_or(""))
}

/// The eight platform colours a `theme.<field>=` token may name.
///
/// They are exactly the fields of `gpui::colors::Colors` — `text`, `selected_text`,
/// `background`, `disabled`, `selected`, `border`, `separator`, `container` — and the
/// list is closed: `theme.accent=` is not a colour this platform has, and it is reported
/// as an unknown token rather than quietly ignored.
pub fn is_theme_field(field: &str) -> bool {
    matches!(
        field,
        "text" | "selected_text" | "background" | "disabled" | "selected" | "border"
            | "separator" | "container"
    )
}

/// The platform colours a style overrides, in the order they appear.
///
/// `theme.background=#222222` replaces one of `gpui::colors::Colors`' fields for the
/// window that draws the element carrying it. Unlike `bg=`, which colours one element,
/// this changes the colour the *platform* answers for that name — what
/// [`crate::view::default_color`] reports and what a framework default is chosen from.
///
/// A token naming a field that does not exist is left to the unknown-token report rather
/// than skipped here, so a misspelling is visible.
pub fn theme_overrides(spec: &str) -> Vec<(String, Hsla)> {
    let mut overrides = Vec::new();
    for (key, value) in tokens(spec) {
        let Some(field) = key.strip_prefix("theme.") else {
            continue;
        };
        let (Some(value), true) = (value, is_theme_field(field)) else {
            continue;
        };
        overrides.push((field.to_string(), colour(&value)));
    }
    overrides
}

/// The tokens a per-range highlight understands.
///
/// A vocabulary of its own, not the element one: `text_styled`'s highlights are
/// `gpui::HighlightStyle`, whose seven properties — colour, background, weight,
/// style, underline, strikethrough and fade — are not the set a `Styled` element
/// takes, and sharing the table would let `p=16` through as a highlight.
fn is_known_highlight(key: &str) -> bool {
    matches!(
        key,
        // the seven properties
        "color" | "bg" | "weight" | "bold" | "semibold" | "italic"
        | "underline" | "strike" | "fade"
        // and the detail each decoration can carry
        | "undercolor" | "underthick" | "underwavy"
        | "strikecolor" | "strikethick"
    )
}

/// The highlight tokens in `props` this module does not understand.
pub fn unknown_highlight_tokens(props: &str) -> Vec<String> {
    tokens(props)
        .into_iter()
        .filter(|(k, _)| !is_known_highlight(k))
        .map(|(k, v)| match v {
            Some(v) => format!("{k}={v}"),
            None => k,
        })
        .collect()
}

/// The interaction tokens in `spec` — `hover=...`, `active=...`, `focus=...`, in that
/// order — if any.
///
/// A refinement is a *second* style string applied while an element is in that state, so
/// its value is itself a style language, and it may be quoted (`hover="bg:#ff0000
/// color=#ffffff"`) because otherwise the space would end the token.
pub fn interaction(spec: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut hover = None;
    let mut active = None;
    let mut focus = None;
    for (key, value) in tokens(spec) {
        match (key.as_str(), value) {
            ("hover", Some(v)) => hover = Some(decode_quoted(&v)),
            ("active", Some(v)) => active = Some(decode_quoted(&v)),
            ("focus", Some(v)) => focus = Some(decode_quoted(&v)),
            _ => {}
        }
    }
    (hover, active, focus)
}

/// The `thumb=<colour>` token in `spec`, if it carries one.
///
/// The colour of a [`crate::view::Kind::Scrollbar`]'s thumb. The element's own `bg=` is its track —
/// one styled box with a bar drawn over it — so a scrollbar is a normal element with one extra
/// token rather than an element with a vocabulary of its own.
pub fn thumb_colour(spec: &str) -> Option<gpui::Hsla> {
    token_colour(spec, "thumb")
}

/// The colour a named token carries, if `spec` sets one.
///
/// A second vocabulary beside `Styled`'s, for the few elements that paint something that is not a
/// box and so cannot be reached by `bg=` or `color=`. [`thumb_colour`] is this with the key fixed to
/// `"thumb"`; the editor reads its own `caret=`/`sel=`/`gutterbg=`/`gutterfg=` the same way. A token
/// with no value, or with an empty one, is *not* a colour and answers `None`, so
/// `ui::editor("doc", "", "gutter")` can carry the token that has no colour without looking like a
/// mistyped one.
pub fn token_colour(spec: &str, key: &str) -> Option<gpui::Hsla> {
    for (name, value) in tokens(spec) {
        if name == key {
            if let Some(value) = value {
                let value = decode_value(&value);
                if !value.is_empty() {
                    return Some(colour(&value));
                }
            }
        }
    }
    None
}

/// The number a named token carries, if `spec` sets one.
///
/// `gutter=32` is a width in pixels. A bare `gutter` has no number, which is how "on, at the default
/// width" is written; a value that is not a number at all is reported to the person the same way a
/// mistyped token is, since a size that silently became zero is the failure mode the token language
/// exists to avoid.
pub fn token_number(spec: &str, key: &str) -> Option<f32> {
    for (name, value) in tokens(spec) {
        if name == key {
            if let Some(value) = value {
                let value = decode_value(&value);
                if !value.is_empty() {
                    return value.parse::<f32>().ok();
                }
            }
        }
    }
    None
}

/// Whether `spec` carries a named bare token.
///
/// `softwrap` is a flag rather than a value: it asks for a behaviour, not a quantity.
pub fn has_flag(spec: &str, key: &str) -> bool {
    tokens(spec).iter().any(|(name, _)| name == key)
}

/// The `ctx=<name>` token in `spec`, if it carries one.
///
/// A key context is what makes a keymap binding focus-scoped: gpui evaluates a
/// binding's context predicate against the context stack of the **focused** node,
/// and this token is how an element puts itself on that stack —
/// `gpui::panel("ctx=Editor")`. It is inert to `Styled`; `view` applies it with
/// `key_context`, and, like `hover=`, it is what makes a plain box interactive
/// enough to be in the dispatch tree at all.
///
/// The value is gpui's context-predicate grammar (`gpui-0.2.2/src/keymap/context.rs`),
/// which is wider than a bare name: `"Editor"`, `"!Editor"`, `"A && B"`, `"A || B"`.
/// Anything it cannot parse is logged by gpui and leaves the element contextless,
/// which is why the value is passed through verbatim rather than validated here.
pub fn key_context(spec: &str) -> Option<String> {
    for (key, value) in tokens(spec) {
        if key == "ctx" {
            if let Some(value) = value {
                let value = decode_value(&value);
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// `Some("x")`, `Some("y")`, or `None`, for the `overflow=` token.///
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

/// Whether `spec` asks for any of the focus-order tokens: `focusable`, `tabstop=` or `tabindex=`.
///
/// A plain box in this binding is a stateless element until it asks to be otherwise, and the focus
/// tokens are applied to interactive elements only. Without this, a box whose style says
/// `focusable tabindex=2 tabstop=1` and nothing else was rendered as a plain box: no id, no
/// handle, no place in the tab order. `19_tab_stop` worked around it by giving every such box a
/// `hover=` refinement that changed nothing, and wrote the workaround down as a visible deviation
/// from upstream. This is the check that lets the workaround go.
///
/// A bare `focusable` counts. So does `focusable=false`, which does *not*: the token's value is
/// read the same way [`crate::view`]'s focus-token pass reads it, so the two cannot disagree about
/// whether an element asked.
pub fn wants_focus(spec: &str) -> bool {
    tokens(spec).into_iter().any(|(key, value)| match key.as_str() {
        "focusable" => value
            .as_deref()
            .map(|v| v != "0" && v != "false")
            .unwrap_or(true),
        "tabstop" | "tabindex" => true,
        _ => false,
    })
}

/// Whether `spec` asks for its element's own box to be reported back.
///
/// The `bounds` token. `gpui::element_bounds(id, field)` answers from a map the *paint* fills, and
/// until this token existed only two kinds of element were put in it: one carrying a pointer
/// listener and one carrying a slider. A box that merely wanted to know how big it came out had
/// nothing to listen with, which made "build a box sized to its content and read back its bounds"
/// unsayable — the whole of `DW11`'s first clause.
///
/// A bare `bounds` counts, and `bounds=0` does not, the same reading [`wants_focus`] gives its own
/// tokens. A div that asks for it gets an *out-of-flow, unpainted, full-size strip* as its first
/// child, because gpui hands a div the bounds of its children and never its own
/// (`Div::on_children_prepainted`, `gpui-0.2.2/src/elements/div.rs:1243`) — the technique round 61
/// found for the pointer rows, reused here for a box that listens to nothing.
///
/// It applies to a `gpui::panel`/`column`/`row`/`stack` box and to `gpui::text`. On an element that
/// already reports — one with a pointer listener or a slider — it changes nothing, because those
/// paths record the same box by the same strip.
pub fn wants_bounds(spec: &str) -> bool {
    tokens(spec).into_iter().any(|(key, value)| match key.as_str() {
        "bounds" => value
            .as_deref()
            .map(|v| v != "0" && v != "false")
            .unwrap_or(true),
        // A group member is read **back as a rectangle** — see [`group`] — so `group=` is the
        // second way to ask for one, and it is spelled here rather than at each call site
        // because a member with no rectangle is a member nothing can answer about.
        "group" => true,
        _ => false,
    })
}

/// The `group=<name>` token in `spec`, if it carries one.
///
/// Names the element as a member of a **group**, which is what `gpui::group_hovered` answers
/// about: a program asks whether any member of a group is under the pointer.
///
/// gpui has the same idea as a *style* — `Interactivity::group_hover` (`elements/div.rs:680`)
/// refines one element while another is hovered — and the binding does not use it, because the
/// registry that would answer for it is `pub(crate)` (`elements/div.rs:2876`, `GroupHitboxes`) and
/// because a style refinement is not something a *program* can read. The reading is the half a
/// program can act on, and it is built out of the one thing the binding does have: the rectangle
/// an element's box is recorded in (`MOUSE_BOUNDS`), which `group=` therefore asks for the same
/// way `bounds` does. So a member is a box with a rectangle, and the answer is the pointer
/// being inside one of them.
///
/// The member needs an `id=` as well: two members of one group are told apart by it, and without
/// one the second member's rectangle overwrites the first's.
pub fn group(spec: &str) -> Option<String> {
    tokens(spec)
        .into_iter()
        .find(|(key, _)| key == "group")
        .and_then(|(_, value)| value)
        .filter(|name| !name.is_empty())
}

/// The `id=<name>` token in `spec`, if it carries one.
///
/// An id is what makes an element addressable from the program: the leaves that
/// take one do so as an argument (`gpui::button(id, ..)`), and every container
/// takes one this way — `gpui::scroll("id=panel overflow=y h=200")`. Without it
/// the arena assigns a per-frame `gpui-auto-N`, which is stable from one frame
/// to the next but which the program cannot name. `id` is otherwise inert: no
/// `Styled` helper answers to it, and [`apply`] ignores it.
pub fn named(spec: &str) -> Option<String> {
    tokens(spec)
        .into_iter()
        .find(|(key, _)| key == "id")
        .and_then(|(_, value)| value)
        .filter(|name| !name.is_empty())
}

/// Split a spec on commas that are not inside brackets.
///
/// `hsla(0, 0.5, 0.5, 0.3)` and `#ffffff` are both one component here.
fn split_components(spec: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in spec.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(&spec[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&spec[start..]);
    out
}

/// Split a spec's first comma-separated component off, keeping brackets whole.
///
/// `hsla(0, 0.5, 0.5, 0.3), 0, -12, 8, 2` is a shadow whose *colour* has commas
/// in it, and a plain `split(',')` reads the hue as the colour and the saturation
/// as the offset. So the colour comes off first, balanced.
fn first_component(spec: &str) -> (&str, &str) {
    let mut depth = 0usize;
    for (i, c) in spec.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return (&spec[..i], &spec[i + 1..]),
            _ => {}
        }
    }
    (spec, "")
}

/// `shadow=<colour>,x,y,blur,spread`, with `|` between several shadows.
fn shadows(v: &str) -> Vec<gpui::BoxShadow> {
    v.split('|')
        .map(|one| {
            let (tint, rest) = first_component(one);
            let parts: Vec<&str> = rest.split(',').collect();
            let at = |i: usize, fallback: f32| {
                parts.get(i).and_then(|p| p.parse::<f32>().ok()).unwrap_or(fallback)
            };
            gpui::BoxShadow {
                color: colour(if tint.is_empty() { "#00000080" } else { tint }),
                offset: gpui::point(px(at(0, 0.0)), px(at(1, 0.0))),
                blur_radius: px(at(2, 0.0)),
                spread_radius: px(at(3, 0.0)),
            }
        })
        .collect()
}

/// `slash=<colour>,width,interval`.
fn slash(v: &str) -> gpui::Background {
    let (tint, rest) = first_component(v);
    let parts: Vec<&str> = rest.split(',').collect();
    let colour = colour(if tint.is_empty() { "#ffffff" } else { tint });
    let width = parts.first().and_then(|w| w.parse::<f32>().ok()).unwrap_or(4.0);
    let interval = parts.get(1).and_then(|i| i.parse::<f32>().ok()).unwrap_or(width);
    gpui::pattern_slash(colour, width, interval)
}

/// A token **value** as the readers below want it.
///
/// [`decode_quoted`] is for *refinements* — `hover=`, `active=`, `focus=` — where a
/// nested `key:value` is shorthand for `key=value` and `_` stands in for the space
/// that cannot be written. A value is not a refinement, and running it through the
/// same decoder quietly broke every `theme:` value read this way:
///
///   * `caret=theme:border` became `theme=border`, which is not a colour this crate
///     knows, so it resolved to the fallback **black**;
///   * `theme:selected_text` — one of the eight field names — became
///     `theme:selected text`, which resolves to nothing for the same reason.
///
/// Both are silent: a colour that resolves to black is indistinguishable from a
/// deliberate black, which is the failure mode the token language exists to avoid,
/// and no note was raised either way. It is why an editor asked for the window's
/// border colour painted its caret black. **Found by running `DW5` in the
/// container**, which is the first run of a GUI probe this project could make
/// without a window on someone's desk — the assertion is one a macOS run had
/// recorded as passing, so the check was believed and the code was not.
fn decode_value(raw: &str) -> String {
    if raw.starts_with("theme:") {
        return raw.to_string();
    }
    decode_quoted(raw)
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
        // `font=` is a size (`2xl`), a family (`Menlo`), or both — `Menlo,2xl`
        // or `Menlo,17`. A family that is not one of the ramp names is a family,
        // because a program that writes `font=Menlo` means the font and not the
        // size five steps up.
        "font" => apply_font(el, v),
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
        // `lh=` is a multiple of the font size; `lhpx=` is an absolute height.
        // A line height is a `DefiniteLength`, which is exactly those two
        // shapes, and a multi-line layout needs the absolute one as often as the
        // relative: a fixed 22-pixel row is a number, not a multiple of whatever
        // font the row happens to inherit.
        "lh" => el.line_height(relative(num(v) as f32)),
        "lhpx" => el.line_height(px(num(v))),
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
        // `Styled::debug()` is `#[cfg(debug_assertions)]` upstream, so it does
        // not exist in a release build and the release build of this crate did
        // not compile. The token stays known either way — a style string that
        // carries `debug` must not be reported as a typo — and draws its border
        // only where gpui can draw one.
        #[cfg(debug_assertions)]
        "debug" => el.debug(),
        #[cfg(not(debug_assertions))]
        "debug" => el,

        _ => el,
    }
}

/// The `font=` token: a size, a family, or both.
///
/// ```text
/// font=2xl             a ramp size
/// font=17              a pixel size — a bare number is not a family name
/// font=Menlo           a family
/// font=Menlo,2xl       a family and a ramp size
/// font=Menlo,17        a family and a pixel size
/// ```
///
/// The family is passed through as written; gpui resolves it against the
/// platform's fonts and falls back if the machine has no such family, so a
/// program that wants to know what is available asks
/// [`gpui::font_families`](crate::font_families).
fn apply_font<T: Styled>(el: T, v: &str) -> T {
    let (family, size) = font_parts(v);

    // With a comma the tail is a size and nothing else. `Menlo,garbage` keeps
    // the family and takes no size — it must not quietly become a request for a
    // font called "garbage", which is what an earlier version of this did.
    if let Some(family) = family.filter(|family| !family.is_empty()) {
        let el = el.font_family(family.to_string());
        return match size.parse::<f32>() {
            Ok(px_size) => el.text_size(px(px_size)),
            Err(_) => match ramp(size, 1.0) {
                Some(_) => text_ramp(el, size),
                None => el,
            },
        };
    }

    // One word: a ramp name, a number, or a family.
    if size.is_empty() {
        return el;
    }
    if ramp(size, 1.0).is_some() {
        return text_ramp(el, size);
    }
    match size.parse::<f32>() {
        Ok(px_size) => el.text_size(px(px_size)),
        Err(_) => el.font_family(size.to_string()),
    }
}

/// The `font=` token split into its family and its size.
///
/// `font=Menlo,2xl` is `(Some("Menlo"), "2xl")`; `font=Menlo` is
/// `(None, "Menlo")`, because a bare word is a family unless a known ramp name
/// says otherwise; `font=,17` is `(Some(""), "17")`, a size with no family.
pub fn font_parts(v: &str) -> (Option<&str>, &str) {
    match v.split_once(',') {
        Some((family, size)) => (Some(family), size),
        None => (None, v),
    }
}

/// A ramp name in rems, or `None` when the string is not one of them.
///
/// These are gpui's own numbers — `text_xs` is `rems(0.75)`, `text_2xl` is
/// `rems(1.5)` — and they are kept here in one place so that a *measurement*
/// resolves `font=2xl` to the same size an element is drawn at. The dispatch to
/// gpui's helpers is [`text_ramp`].
pub fn ramp(name: &str, rem: f32) -> Option<f32> {
    let multiple = match name {
        "xs" => 0.75,
        "sm" => 0.875,
        "base" => 1.0,
        "lg" => 1.125,
        "xl" => 1.25,
        "2xl" => 1.5,
        "3xl" => 1.875,
        _ => return None,
    };
    Some(rem * multiple)
}

/// The gpui helper behind each ramp name.
fn text_ramp<T: Styled>(el: T, name: &str) -> T {
    match name {
        "xs" => el.text_xs(),
        "sm" => el.text_sm(),
        "base" => el.text_base(),
        "lg" => el.text_lg(),
        "xl" => el.text_xl(),
        "2xl" => el.text_2xl(),
        "3xl" => el.text_3xl(),
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

thread_local! {
    /// The colour table a `theme:<name>` value resolves against, one per window.
    ///
    /// `view::begin_frame` fills it — and replaces it every frame — from the same
    /// `Colors` that `gpui::default_color` answers from, so a token and the reader cannot
    /// disagree. The language lives here and the window lives in `view`, which is why the
    /// vocabulary is a table this module is handed rather than one it goes and reads.
    static THEMED: RefCell<HashMap<i64, HashMap<String, Hsla>>> =
        RefCell::new(HashMap::new());
}

/// Records the frame's colour table for `win`, replacing whatever the last frame left.
pub fn set_colour_table(win: i64, table: HashMap<String, Hsla>) {
    THEMED.with(|t| {
        t.borrow_mut().insert(win, table);
    });
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, or one of GPUI's named colours.
///
/// A `theme:<name>` value resolves against this window's table instead. Anything this does not
/// recognise draws **black** rather than failing — which is why [`unknown_tokens`] exists and why
/// every element that takes a style string is checked by it: a colour typo has to be visible as a
/// complaint, because as a colour it is indistinguishable from a deliberate black.
pub fn colour(v: &str) -> Hsla {
    // `hsla(<h>,<s>,<l>,<a>)` — gpui's own `hsla`, argument for argument, and
    // the spelling that keeps an alpha **exact**.
    //
    // A hex colour is `#rrggbbaa`, so its alpha is a byte: 26/255 is not 0.1 and
    // 13/255 is not 0.05, and gpui blends with the f32 a colour carries. A port
    // that writes upstream's `green().opacity(0.1)` as `#0080001a` therefore
    // hands the renderer a different colour from the one upstream hands it, and
    // the difference survives to the screen: `16_scrollable`'s captures differ in
    // 63,726 pixels, 63,263 of them the single green box — (212,228,225) in
    // upstream's capture against (211,226,225) in the port's, over a fill that
    // looks identical in both.
    //
    // `h` is in turns, as gpui's is: `hsla(0.3333333333, 1, 0.25, 0.1)` is
    // `green().opacity(0.1)` exactly. Three arguments take an alpha of 1.
    if let Some(rest) = v.strip_prefix("hsla(").and_then(|rest| rest.strip_suffix(')')) {
        let mut parts = rest.split(',');
        let mut num = |fallback: f32| {
            parts
                .next()
                .and_then(|p| p.trim().parse::<f32>().ok())
                .unwrap_or(fallback)
        };
        let (h, s, l) = (num(0.0), num(0.0), num(0.0));
        let a = num(1.0);
        return hsla(h, s, l, a);
    }
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
    if let Some(name) = v.strip_prefix("theme:") {
        // The frame's table: the platform's palette, the theme `set_theme` chose, and
        // any `theme.<field>=` overrides. Filled once per frame by `view::begin_frame`;
        // empty only where there is no window at all.
        return THEMED.with(|t| {
            t.borrow()
                .get(&window::current())
                .and_then(|table| table.get(name))
                .copied()
        })
        .unwrap_or_else(|| rgb(0x000000).into());
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
    // Components, not a plain `split(',')`: a colour may be an `hsla(..)`, whose
    // own commas are inside brackets.
    let parts = split_components(v);
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
