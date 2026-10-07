//! Lines, carets and words: the arithmetic of a text, with no window in it.
//!
//! Three rows stand on this — DW1's caret moves, DW4's double-click word and triple-click line, and
//! DW5's gutter — and none of the three can be finished without a window, because the layout that
//! draws them and the measurement that turns a point into an offset both live on one. What *can* be
//! had without a window is the arithmetic, and it is worth having on its own: a Gossamer program
//! keeps its text in the store (round 48 showed the store **is** the document), and until now it
//! could not ask where a line begins, what word is under an offset, or where the caret lands one
//! line down.
//!
//! **Offsets are UTF-8 byte offsets**, the same as `gpui::styled_text`'s highlight ranges. Every
//! answer is snapped down to a character boundary, so an offset in the middle of a multi-byte
//! character gets a whole one rather than a broken one, and an offset past the end of the text is
//! clamped rather than refused: a caret that has run off the end belongs at the end.

/// Snaps `offset` down to the nearest character boundary, and into the text.
fn boundary(text: &str, offset: usize) -> usize {
    let mut at = offset.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The byte range of every line, **excluding** the break that ends it.
///
/// A text ending in a break has a last line that is empty and starts after it, which is what makes
/// `"ab\n"` two lines rather than one.
fn lines_of(text: &str) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if character == '\n' {
            lines.push((start, index));
            start = index + 1;
        }
    }
    lines.push((start, text.len()));
    lines
}

/// The line `offset` is on, counting from zero.
///
/// The break itself belongs to the line it ends, so an offset on a `\n` answers the line that break
/// closes rather than the one after it.
fn index_of_line(text: &str, offset: usize) -> usize {
    let at = boundary(text, offset);
    let lines = lines_of(text);
    for (index, (start, end)) in lines.iter().enumerate() {
        if at >= *start && at <= *end {
            return index;
        }
    }
    lines.len() - 1
}

/// How many lines the text has. A text with no break is one line; so is an empty text.
pub fn lines(text: &str) -> i64 {
    lines_of(text).len() as i64
}

/// The line `offset` is on, counting from zero.
pub fn line_of(text: &str, offset: usize) -> i64 {
    index_of_line(text, offset) as i64
}

/// The byte range of the line `offset` is on, as `"<start>,<end>"`, excluding its break.
pub fn line_bounds(text: &str, offset: usize) -> String {
    let (start, end) = lines_of(text)[index_of_line(text, offset)];
    format!("{start},{end}")
}

/// Where the caret lands `lines` lines from `offset`, aiming for `column` bytes into the target line.
///
/// Negative is up and positive is down, so a page move is one call with a bigger number. The aim is
/// clamped into the target line — never past its end, never across its break — so a caret moving
/// down a short line stops at the end of it, which is what every editor does. Moving up from the
/// first line or down from the last stays on the line it is already on.
///
/// The **column is the caller's to keep**: this is a pure function and remembers nothing, so a
/// program that wants the caret to come back to the column it left from passes the same number
/// again, and one that wants a plain move passes what it likes.
pub fn line_move(text: &str, offset: usize, lines: i64, column: i64) -> i64 {
    let all = lines_of(text);
    let current = index_of_line(text, offset) as i64;
    let target = (current + lines).clamp(0, all.len() as i64 - 1) as usize;
    let (start, end) = all[target];
    let aim = start.saturating_add(column.max(0) as usize).min(end);
    boundary(text, aim).max(start) as i64
}

/// Whether `character` counts as part of a word: a letter, a digit or an underscore.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The word `offset` is on, as `"<start>,<end>"`.
///
/// A word is a run of letters, digits and underscores. **Anything else is not a word**, and the
/// answer is the single character under the offset — so a double-click on a comma takes the comma
/// and not the run of punctuation and spaces around it. Past the end of the text the answer is the
/// empty word at the end, and an empty text answers `"0,0"`.
pub fn word_at(text: &str, offset: usize) -> String {
    let end_of_text = text.len();
    if text.is_empty() {
        return "0,0".to_string();
    }
    let at = boundary(text, offset);
    if at >= end_of_text {
        return format!("{end_of_text},{end_of_text}");
    }
    let under = text[at..]
        .chars()
        .next()
        .expect("a boundary inside a non-empty text has a character after it");
    if !is_word_character(under) {
        return format!("{at},{}", at + under.len_utf8());
    }
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_word_character(*character))
        .last()
        .map(|(index, _)| index)
        .unwrap_or(at);
    let end = text[at..]
        .char_indices()
        .take_while(|(_, character)| is_word_character(*character))
        .map(|(index, character)| at + index + character.len_utf8())
        .last()
        .unwrap_or(at + under.len_utf8());
    format!("{start},{end}")
}
