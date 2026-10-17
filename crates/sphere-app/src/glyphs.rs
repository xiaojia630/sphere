//! Symbol fallback: Rajdhani and Michroma lack arrows, triangles, ticks and the like, so a run of those in a text
//! in a UI font is drawn in the mono font instead (as `TextSpan` children of the text), at a size that matches.
use crate::fonts::UiFonts;
use bevy::prelude::*;
use bevy::text::{FontSource, LetterSpacing, LineHeight};

/// Characters the UI fonts lack and JetBrains Mono mostly has (not ✓ ✗ ○ ↵ ↹: avoid those).
pub fn is_sym(c: char) -> bool {
    matches!(c, '\u{2039}' | '\u{203A}' | '\u{2190}'..='\u{21FF}' | '\u{2300}'..='\u{23FF}' | '\u{25A0}'..='\u{25FF}' | '\u{2600}'..='\u{27BF}')
}

/// Splits into runs: (is symbol run, text).
pub fn runs(s: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut cur: Option<bool> = None;
    for (i, c) in s.char_indices() {
        // a space after a symbol stays with it, drawn at the mono width
        let k = is_sym(c) || (c == ' ' && cur == Some(true));
        if cur.is_some_and(|p| p != k) {
            out.push((cur.unwrap(), &s[start..i]));
            start = i;
        }
        cur = Some(k);
    }
    if let Some(k) = cur {
        out.push((k, &s[start..]));
    }
    out
}

/// The mono spans of a text, so they can be dropped when the text changes.
#[derive(Component)]
pub struct SymSpans(Vec<Entity>);

/// Mono glyphs look larger than Rajdhani's at one size.
const SYM_SCALE: f32 = 0.9;

#[allow(clippy::type_complexity)]
pub fn fallback(
    mut c: Commands,
    f: Res<UiFonts>,
    mut q: Query<(Entity, &mut Text, &TextFont, &LetterSpacing, &TextColor, &LineHeight, Option<&SymSpans>), (Changed<Text>, Without<TextSpan>)>,
    recolor: Query<(&TextColor, &SymSpans), (Changed<TextColor>, Without<TextSpan>)>,
    mut spans: Query<&mut TextColor, With<TextSpan>>,
) {
    for (e, mut text, tf, ls, col, lh, old) in &mut q {
        let mono = |h: &FontSource| matches!(h, FontSource::Handle(h) if *h == f.mono || *h == f.mono_bold);
        if mono(&tf.font) {
            continue;
        }
        let has = text.0.chars().any(is_sym);
        if !has && old.is_none() {
            continue;
        }
        if let Some(old) = old {
            for s in &old.0 {
                if let Ok(mut ec) = c.get_entity(*s) {
                    ec.despawn();
                }
            }
            c.entity(e).remove::<SymSpans>();
        }
        if !has {
            continue;
        }
        let s = std::mem::take(&mut text.0);
        let mut ids = Vec::new();
        let mut first = true;
        let FontSize::Px(size) = tf.font_size else { text.0 = s; continue };
        for (sym, run) in runs(&s) {
            if first && !sym {
                text.0 = run.to_string();
                first = false;
                continue;
            }
            first = false;
            let mut font = tf.clone();
            let mut spacing = *ls;
            if sym {
                font.font = FontSource::Handle(f.mono.clone());
                font.font_size = FontSize::Px(size * SYM_SCALE);
                spacing = LetterSpacing::Px(0.0);
            }
            ids.push(c.spawn((TextSpan::new(run), font, spacing, *col, *lh, ChildOf(e))).id());
        }
        c.entity(e).insert(SymSpans(ids));
    }
    for (col, sp) in &recolor {
        for s in &sp.0 {
            if let Ok(mut t) = spans.get_mut(*s) {
                *t = *col;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_runs() {
        assert_eq!(runs("a→b"), vec![(false, "a"), (true, "→"), (false, "b")]);
        assert_eq!(runs("↑↓ move"), vec![(true, "↑↓ "), (false, "move")]);
        assert_eq!(runs("↑ ↓ SELECT"), vec![(true, "↑ ↓ "), (false, "SELECT")]);
        assert_eq!(runs("plain"), vec![(false, "plain")]);
        assert!(runs("").is_empty());
    }

    #[test]
    fn mono_has_every_symbol_and_the_ui_fonts_lack_them() {
        let mono = fontdue::Font::from_bytes(crate::fonts::MONO_400, fontdue::FontSettings::default()).unwrap();
        let ui = fontdue::Font::from_bytes(crate::fonts::RAJDHANI_500, fontdue::FontSettings::default()).unwrap();
        for ch in "←→↑↓▲▼‹›◆▸◂●■□⌘⇧⌃⌥".chars() {
            assert!(is_sym(ch), "{ch}");
            assert!(mono.lookup_glyph_index(ch) != 0, "mono lacks {ch}");
        }
        for ch in "←→↑↓".chars() {
            assert_eq!(ui.lookup_glyph_index(ch), 0, "ui has {ch}");
        }
        for ch in "·…–—×".chars() {
            assert!(!is_sym(ch));
        }
    }
}
