// SPDX-License-Identifier: FSL-1.1-ALv2
//! BCP 47 (RFC 5646) language tag syntax validation and comparison.
//!
//! Only well-formedness is checked, not registry membership: the model decides whether it can
//! translate a well-formed tag. Tags compare case-insensitively, so `en-US` and `EN-us` are the
//! same language for the identical-language rule.

const MAX_TAG_BYTES: usize = 64;

/// RFC 5646 irregular grandfathered tags, which do not match the regular grammar.
const IRREGULAR: [&str; 7] = [
    "en-gb-oed",
    "i-ami",
    "i-bnn",
    "i-default",
    "i-enochian",
    "i-hak",
    "i-klingon",
];
const IRREGULAR_MORE: [&str; 10] = [
    "i-lux",
    "i-mingo",
    "i-navajo",
    "i-pwn",
    "i-tao",
    "i-tay",
    "i-tsu",
    "sgn-be-fr",
    "sgn-be-nl",
    "sgn-ch-de",
];

fn alpha(part: &str) -> bool {
    part.bytes().all(|b| b.is_ascii_alphabetic())
}
fn digit(part: &str) -> bool {
    part.bytes().all(|b| b.is_ascii_digit())
}
fn alnum(part: &str) -> bool {
    part.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Normalize a tag for comparison, or `None` when it is not well-formed BCP 47.
pub fn normalize(tag: &str) -> Option<String> {
    if tag.is_empty() || tag.len() > MAX_TAG_BYTES || !tag.is_ascii() {
        return None;
    }
    let lower = tag.to_ascii_lowercase();
    if IRREGULAR.contains(&lower.as_str()) || IRREGULAR_MORE.contains(&lower.as_str()) {
        return Some(lower);
    }
    let parts: Vec<&str> = lower.split('-').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || p.len() > 8 || !alnum(p))
    {
        return None;
    }
    // Private use only: x-...
    if parts[0] == "x" {
        return (parts.len() > 1).then_some(lower);
    }
    let mut index = 0;
    // language: 2-3 letters, optionally followed by up to three 3-letter extlangs; 4 letters
    // (reserved for future use, still well-formed); 5-8 letters (registered).
    let language = parts[index];
    if !alpha(language) || !(2..=8).contains(&language.len()) {
        return None;
    }
    index += 1;
    if language.len() <= 3 {
        let mut extlangs = 0;
        while extlangs < 3 && parts.get(index).is_some_and(|p| p.len() == 3 && alpha(p)) {
            index += 1;
            extlangs += 1;
        }
    }
    if parts.get(index).is_some_and(|p| p.len() == 4 && alpha(p)) {
        index += 1;
    }
    if parts
        .get(index)
        .is_some_and(|p| (p.len() == 2 && alpha(p)) || (p.len() == 3 && digit(p)))
    {
        index += 1;
    }
    // variants: 5-8 alphanumerics or a digit followed by three alphanumerics
    while parts.get(index).is_some_and(|p| {
        (5..=8).contains(&p.len()) || (p.len() == 4 && p.as_bytes()[0].is_ascii_digit())
    }) {
        index += 1;
    }
    // extensions: a singleton other than x followed by 2-8 character subtags
    while let Some(singleton) = parts.get(index) {
        if singleton.len() != 1 || *singleton == "x" {
            break;
        }
        index += 1;
        let start = index;
        while parts.get(index).is_some_and(|p| (2..=8).contains(&p.len())) {
            index += 1;
        }
        if index == start {
            return None;
        }
    }
    if parts.get(index) == Some(&"x") {
        index += 1;
        if index >= parts.len() {
            return None;
        }
        index = parts.len();
    }
    (index == parts.len()).then_some(lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_tags_are_accepted_case_insensitively() {
        for tag in [
            "en",
            "en-US",
            "EN-us",
            "ko",
            "english",
            "zh-Hant-TW",
            "zh-cmn-Hans-CN",
            "sr-Latn-RS",
            "es-419",
            "de-CH-1901",
            "sl-rozaj-biske",
            "en-a-bbb-x-a-ccc",
            "x-private",
            "i-klingon",
            "fil",
            "ja-JP-u-ca-japanese",
            "yue-HK",
            "und",
            // RFC 5646 reserves four-letter primary subtags; they are well-formed.
            "abcd",
            "ABCD-Latn-US",
        ] {
            assert!(normalize(tag).is_some(), "{tag}");
        }
        assert_eq!(normalize("EN-us"), normalize("en-US"));
        assert_ne!(normalize("en"), normalize("en-US"));
    }

    #[test]
    fn malformed_tags_are_rejected() {
        for tag in [
            "",
            "e",
            "en_US",
            "en-",
            "-en",
            "en--US",
            "en-US-US-US-US-US-US-US-US-US-US-US-US-US-US-US-US-US-US-US",
            "1n",
            "en US",
            "en-a",
            "en-x",
            "x",
            "toolongsubtag-en",
            "한국어",
            "en-US\n",
            "abcd-efgh-ijkl-mnop-qrst",
            "en-a-b",
        ] {
            assert!(normalize(tag).is_none(), "{tag:?}");
        }
    }
}
