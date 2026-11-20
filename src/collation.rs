// Copyright 2024 KipData/KiteSQL
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! MySQL 8's default collation, `utf8mb4_0900_ai_ci`, as a comparison key.
//!
//! Strings are equal when they differ only in case, accents, compatibility forms (`'ﬁ'`,
//! `'Ａ'`, `'①'`) or ignorable characters (U+0000, soft hyphen, zero-width characters), and
//! a few letters expand as UCA does (`'ß'` = `'ss'`, `'æ'` = `'ae'`, `'ø'` = `'o'`). Like
//! MySQL's 0900 collations it is NO PAD: `'a'` and `'a '` differ.
//!
//! The key orders letters and digits as MySQL does (`'a'` < `'B'`, `'é'` < `'f'`), but
//! punctuation and symbols keep their code point order rather than the UCA's (where all
//! punctuation sorts before digits), and there are no contractions or per-language rules.

use kite_sql::types::collation::TextCollation;
use std::borrow::Cow;

/// The name tables created under this collation are marked with.
pub const UTF8MB4_0900_AI_CI: &str = "utf8mb4_0900_ai_ci";

pub fn utf8mb4_0900_ai_ci() -> TextCollation {
    TextCollation::new(UTF8MB4_0900_AI_CI, key)
}

/// Whether a collation name compares byte-wise or case-sensitively (`utf8mb4_bin`,
/// `utf8mb4_0900_as_cs`, `binary`, `latin1_bin`).
pub fn is_binary_collation(name: &str) -> bool {
    let name = name.trim_matches(|c| c == '`' || c == '"' || c == '\'');
    let name = name.to_ascii_lowercase();
    name == "binary" || name.ends_with("_bin") || name.ends_with("_cs")
}

/// Whether a collation name is accent-sensitive but case-insensitive (`utf8mb4_0900_as_ci`).
pub fn is_accent_sensitive_collation(name: &str) -> bool {
    let name = name.trim_matches(|c| c == '`' || c == '"' || c == '\'');
    name.to_ascii_lowercase().ends_with("_as_ci")
}

/// The comparison key: equal keys for strings the collation treats as equal.
///
/// The key is idempotent (`key(key(s)) == key(s)`): tables keep it in their keys.
pub fn key(text: &str) -> Cow<'_, str> {
    if text.is_ascii() {
        if !text.bytes().any(|b| b.is_ascii_uppercase() || b == 0) {
            return Cow::Borrowed(text);
        }
        return Cow::Owned(
            text.bytes()
                .filter(|b| *b != 0)
                .map(|b| b.to_ascii_lowercase() as char)
                .collect(),
        );
    }
    let mut lowered = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            // NFKD would leave the middle dot: UCA folds the whole letter to `l`.
            'ŀ' | 'Ŀ' => lowered.push('l'),
            // Lowercasing it would add a combining dot.
            'İ' => lowered.push('i'),
            c => lowered.extend(c.to_lowercase()),
        }
    }
    let decomposed = icu_normalizer::DecomposingNormalizerBorrowed::new_nfkd().normalize(&lowered);
    let mut out = String::with_capacity(decomposed.len());
    for c in decomposed.chars() {
        if is_combining_mark(c) {
            // The breve of `й` makes a letter of its own; other accents are ignored.
            if c == '\u{0306}' && out.ends_with('и') {
                out.pop();
                out.push('й');
            }
            continue;
        }
        match c {
            '\u{0000}' | '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}' => {}
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            'ø' => out.push('o'),
            'đ' | 'ð' => out.push('d'),
            'ł' => out.push('l'),
            'ħ' => out.push('h'),
            'ς' => out.push('σ'),
            c => out.extend(c.to_lowercase()),
        }
    }
    Cow::Owned(out)
}

/// Nonspacing combining marks, as NFKD leaves them after a base letter.
fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{0483}'..='\u{0489}'
            | '\u{0591}'..='\u{05BD}'
            | '\u{05BF}'
            | '\u{05C1}'..='\u{05C2}'
            | '\u{05C4}'..='\u{05C5}'
            | '\u{05C7}'
            | '\u{0610}'..='\u{061A}'
            | '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_under_mysql() {
        for (a, b) in [
            ("a", "A"),
            ("é", "e"),
            ("ß", "ss"),
            ("straße", "STRASSE"),
            ("ǅ", "dž"),
            ("Å", "a"),
            ("æ", "ae"),
            ("œ", "oe"),
            ("ø", "o"),
            ("đ", "d"),
            ("ł", "l"),
            ("Ǆ", "DŽ"),
            ("ﬁ", "fi"),
            ("①", "1"),
            ("Ａ", "a"),
            ("ð", "d"),
            ("İ", "i"),
            ("ŀ", "l"),
            ("ħ", "h"),
            ("ĳ", "ij"),
            ("ſ", "s"),
            ("ǈ", "lj"),
            ("Ð", "d"),
            ("Ø", "o"),
            ("Æ", "ae"),
            ("²", "2"),
            ("Ω", "ω"),
            ("Σ", "ς"),
            ("σ", "ς"),
            ("ё", "е"),
            ("a\0", "a"),
            ("a\u{00AD}b", "ab"),
            ("Éclair", "eclair"),
        ] {
            assert_eq!(key(a), key(b), "{a:?} = {b:?}");
        }
    }

    #[test]
    fn different_under_mysql() {
        for (a, b) in [
            ("a", "a "),
            ("😀", "😁"),
            ("ı", "i"),
            ("þ", "th"),
            ("ŧ", "t"),
            ("ŉ", "n"),
            ("ŋ", "n"),
            ("ə", "e"),
            ("ĸ", "k"),
            ("й", "и"),
            ("½", "1/2"),
            ("a\tb", "ab"),
            ("\u{1}", ""),
            ("", " "),
        ] {
            assert_ne!(key(a), key(b), "{a:?} <> {b:?}");
        }
    }

    #[test]
    fn orders_like_mysql() {
        for (a, b) in [
            ("a", "B"),
            ("é", "f"),
            ("_", "a"),
            ("1", "a"),
            ("ss", "st"),
            ("ß", "st"),
            ("", " "),
            (" ", "a"),
            ("a", "a "),
            ("ä", "Z"),
        ] {
            assert!(key(a) < key(b), "{a:?} < {b:?}");
        }
    }

    #[test]
    fn key_is_idempotent() {
        for text in ["Straße", "Éclair", "Йод", "ǅ", "İ", "ŀ", "ﬁ", "Σσς", "a\0b"] {
            let once = key(text).into_owned();
            assert_eq!(key(&once), once, "{text:?}");
        }
    }

    #[test]
    fn binary_collation_names() {
        assert!(is_binary_collation("utf8mb4_bin"));
        assert!(is_binary_collation("utf8mb4_0900_as_cs"));
        assert!(is_binary_collation("binary"));
        assert!(!is_binary_collation("utf8mb4_0900_ai_ci"));
        assert!(!is_binary_collation("utf8mb4_0900_as_ci"));
        assert!(is_accent_sensitive_collation("utf8mb4_0900_as_ci"));
    }
}
