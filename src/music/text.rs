// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! Names: what a song, playlist or mod may be called, and the asset slugs derived from them.

use std::collections::BTreeSet;

/// Whether text is fit for an artist, title or playlist name: 1-255 bytes, no control characters.
pub fn usable_name(text: &str) -> bool {
    !text.is_empty() && text.len() <= 255 && !text.bytes().any(|byte| byte < 32 || byte == 127)
}

/// Smart quote / apostrophe sequences folded onto ASCII. The trailing rows are the
/// double-encoded ("â€™") forms that mis-tagged files produce for the same characters.
const QUOTE_FOLDS: &[(&str, char)] = &[
    ("\u{2018}", '\''),
    ("\u{2019}", '\''),
    ("\u{201a}", '\''),
    ("\u{2032}", '\''),
    ("\u{201c}", '"'),
    ("\u{201d}", '"'),
    ("\u{201e}", '"'),
    ("\u{e2}\u{20ac}\u{2122}", '\''),
    ("\u{e2}\u{20ac}\u{2dc}", '\''),
    ("\u{e2}\u{20ac}\u{201a}", '\''),
    ("\u{e2}\u{20ac}\u{2032}", '\''),
    ("\u{e2}\u{20ac}\u{153}", '"'),
    ("\u{e2}\u{20ac}\u{9d}", '"'),
    ("\u{e2}\u{20ac}\u{201e}", '"'),
];

/// Folds smart quotes and UTF-8/mojibake apostrophes onto their ASCII forms, so a tag cannot
/// smuggle multi-byte sequences into an asset name or the song id.
pub fn sanitize_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while let Some(first) = rest.chars().next() {
        for &(from, to) in QUOTE_FOLDS {
            if let Some(after) = rest.strip_prefix(from) {
                out.push(to);
                rest = after;
                continue 'outer;
            }
        }
        out.push(first);
        rest = &rest[first.len_utf8()..];
    }
    out
}

/// The asset-path slug of a song: ASCII letters and digits only, since it becomes an asset path.
/// Smart quotes are folded first, and every other run of punctuation collapses to one underscore.
pub fn slug_of(artist: &str, title: &str) -> String {
    let source = format!("{}_{}", sanitize_text(artist), sanitize_text(title));
    let mut slug = String::new();
    for byte in source.bytes() {
        if byte.is_ascii_alphanumeric() {
            slug.push(byte as char);
        } else if !slug.is_empty() && !slug.ends_with('_') {
            slug.push('_');
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    slug.truncate(64);
    if slug.is_empty() {
        slug = "Song".into();
    }
    slug
}

/// Case-insensitive uniqueness for asset slugs: returns `base`, or base_2, base_3, ... until the
/// lowered result is unused, and records it in `used`. The bundle stores asset names lowercased,
/// so two titles differing only in case must not resolve to the same asset.
pub fn unique_slug(base: &str, used: &mut BTreeSet<String>) -> String {
    let mut candidate = base.to_owned();
    let mut n = 2;
    while !used.insert(candidate.to_ascii_lowercase()) {
        candidate = format!("{base}_{n}");
        n += 1;
    }
    candidate
}

/// Trailing whitespace and leading spaces/tabs removed, as tags are read.
pub fn trim_tag(text: &str) -> String {
    text.trim_end_matches(|c: char| c.is_ascii_whitespace()).trim_start_matches([' ', '\t']).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_keep_ascii_words_and_collapse_the_rest() {
        assert_eq!(slug_of("Diego Money", "Wit Em"), "Diego_Money_Wit_Em");
        assert_eq!(slug_of("AC/DC", "T.N.T."), "AC_DC_T_N_T");
        assert_eq!(slug_of("!!!", "???"), "Song");
        assert_eq!(slug_of(&"a".repeat(80), "b").len(), 64);
    }

    #[test]
    fn case_only_collisions_get_a_suffix() {
        let mut used = BTreeSet::new();
        let first = slug_of("Diego Money", "Wit Em");
        let second = slug_of("DIEGO MONEY", "WIT EM");
        assert_eq!(unique_slug(&first, &mut used), "Diego_Money_Wit_Em");
        let deduped = unique_slug(&second, &mut used);
        assert_eq!(deduped, "DIEGO_MONEY_WIT_EM_2");
        assert_eq!(format!("{}_mg", deduped.to_lowercase()), "diego_money_wit_em_2_mg");
        assert_eq!(unique_slug("diego_money_wit_em", &mut used), "diego_money_wit_em_3");
    }

    #[test]
    fn smart_quotes_fold_to_ascii() {
        assert_eq!(sanitize_text("P\u{2019}Z"), "P'Z");
        assert_eq!(sanitize_text("\u{e2}\u{20ac}\u{2122}"), "'");
        assert_eq!(sanitize_text("\u{201c}Hi\u{201d}"), "\"Hi\"");
        assert_eq!(slug_of("P\u{2019}Z", "x"), slug_of("P'Z", "x"));
        assert_eq!(sanitize_text("Beyonc\u{e9}"), "Beyonc\u{e9}");
    }

    #[test]
    fn names_must_be_short_and_printable() {
        assert!(usable_name("My Mix"));
        assert!(!usable_name(""));
        assert!(!usable_name("tab\there"));
        assert!(!usable_name(&"x".repeat(256)));
    }

    #[test]
    fn tags_are_trimmed() {
        assert_eq!(trim_tag("  Artist \r\n"), "Artist");
    }
}
