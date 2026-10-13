//! The compose window's formatting: a light markup (`**bold**`, `*italic*`, `__underline__`,
//! `- item`, `1. item`, `[text](url)`, `![name](cid:…)`) applied to the selection of the body
//! and rendered as HTML when the message goes out (`core::mime::markup_to_html`).

use egui::text::{CCursor, CCursorRange};

/// The selected character range of a text edit, ordered.
pub fn selection(ctx: &egui::Context, id: egui::Id) -> Option<(usize, usize)> {
    let state = egui::TextEdit::load_state(ctx, id)?;
    let r = state.cursor.char_range()?;
    let (a, b) = (r.primary.index.0, r.secondary.index.0);
    Some((a.min(b), a.max(b)))
}

/// Puts the caret at a character index and gives the text edit the focus back.
pub fn set_caret(ctx: &egui::Context, id: egui::Id, index: usize) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(index))));
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

fn byte_at(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(s.len())
}

/// Wraps the selection (or inserts an empty pair at the caret) in `prefix`/`suffix`; returns
/// the character index the caret should go to (after the wrapped text, or inside the pair).
pub fn wrap(text: &mut String, sel: Option<(usize, usize)>, prefix: &str, suffix: &str) -> usize {
    let (a, b) = sel.unwrap_or((text.chars().count(), text.chars().count()));
    let (ba, bb) = (byte_at(text, a), byte_at(text, b));
    let inner = text.get(ba..bb).unwrap_or_default().to_string();
    let replacement = format!("{prefix}{inner}{suffix}");
    text.replace_range(ba..bb, &replacement);
    if inner.is_empty() { a + prefix.chars().count() } else { a + replacement.chars().count() }
}

/// Starts every line touched by the selection (or the caret's line) as a list item.
pub fn list(text: &mut String, sel: Option<(usize, usize)>, numbered: bool) -> usize {
    let (a, b) = sel.unwrap_or((text.chars().count(), text.chars().count()));
    let (ba, bb) = (byte_at(text, a), byte_at(text, b));
    let start = text.get(..ba).and_then(|s| s.rfind('\n')).map_or(0, |i| i + 1);
    let end = text.get(bb..).and_then(|s| s.find('\n')).map_or(text.len(), |i| bb + i);
    let block = text.get(start..end).unwrap_or_default().to_string();
    let lines: Vec<String> = block
        .split('\n')
        .enumerate()
        .map(|(i, l)| {
            let bare = l.trim_start_matches("- ").trim_start_matches("* ");
            let bare = bare.find(". ").filter(|i| *i > 0 && bare[..*i].chars().all(|c| c.is_ascii_digit())).map_or(bare, |i| &bare[i + 2..]);
            if numbered { format!("{}. {bare}", i + 1) } else { format!("- {bare}") }
        })
        .collect();
    let replacement = lines.join("\n");
    text.replace_range(start..end, &replacement);
    text.get(..start).map_or(0, |s| s.chars().count()) + replacement.chars().count()
}

/// `[selection](https://)` with the caret ready for the address.
pub fn link(text: &mut String, sel: Option<(usize, usize)>) -> usize {
    let (a, b) = sel.unwrap_or((text.chars().count(), text.chars().count()));
    let (ba, bb) = (byte_at(text, a), byte_at(text, b));
    let inner = text.get(ba..bb).unwrap_or_default().to_string();
    let label = if inner.is_empty() { "link".to_string() } else { inner };
    let replacement = format!("[{label}](https://)");
    text.replace_range(ba..bb, &replacement);
    a + replacement.chars().count() - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_is_applied_to_the_selection() {
        let mut t = "Ciao Luca, è urgente".to_string();
        assert_eq!(wrap(&mut t, Some((5, 9)), "**", "**"), 13);
        assert_eq!(t, "Ciao **Luca**, è urgente");
        assert_eq!(wrap(&mut t, None, "*", "*"), t.chars().count() - 1);
        assert!(t.ends_with("urgente**"));
        let mut l = "uno\ndue\ntre".to_string();
        assert_eq!(list(&mut l, Some((0, 6)), false), 11);
        assert_eq!(l, "- uno\n- due\ntre");
        list(&mut l, Some((0, 14)), true);
        assert_eq!(l, "1. uno\n2. due\n3. tre");
        let mut k = "vedi qui".to_string();
        assert_eq!(link(&mut k, Some((5, 8))), 19);
        assert_eq!(k, "vedi [qui](https://)");
    }
}
