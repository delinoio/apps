//! Pinned offline keyboards and dictionaries for additional languages.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::OnceLock,
};

use serde::Deserialize;

use crate::{Language, LanguageSet, decision::Validation};

/// Dictionary identity and the canonical public keyboard used with it.
#[derive(Debug, Deserialize)]
pub struct LanguageInfo {
    pub id: String,
    pub locale: String,
    pub layout: String,
    pub dictionary: String,
    pub name: String,
    pub license: String,
    pub validation: DictionaryMode,
}

/// Exact capabilities of the pinned resource, independent of language names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DictionaryMode {
    Hunspell,
    ListedWords,
}

#[derive(Deserialize)]
pub struct Keyboard {
    keys: BTreeMap<char, String>,
    transforms: BTreeMap<String, String>,
    dead: Vec<String>,
}

#[derive(Deserialize)]
pub struct Catalog {
    pub languages: Vec<LanguageInfo>,
    layouts: BTreeMap<String, Keyboard>,
}

/// Returns the checked-in catalog. Its shape and ordering are tested against
/// the generated enum and resource checksums before distribution.
///
/// # Panics
///
/// Panics if the compiled-in catalog is not valid JSON.
#[must_use]
pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/languages/catalog.json"))
            .expect("the bundled language catalog must be valid")
    })
}

fn keyboard(language: Language) -> Option<&'static Keyboard> {
    let Language::Additional(index) = language else {
        return None;
    };
    let catalog = catalog();
    catalog
        .layouts
        .get(&catalog.languages[index as usize].layout)
}

/// Encodes a level-three (`AltGr`) physical key without exposing rendered text.
#[must_use]
pub fn level_three(key: char) -> Option<char> {
    char::from_u32(u32::from(key) + if key >= '\u{e000}' { 0x100 } else { 0xe000 })
}

/// Replays a keyboard including literal dead-key transforms. Unknown keys and
/// incomplete dead-key sequences cannot authorize automatic replacement.
#[must_use]
pub fn render(keys: &[char], language: Language, partial: bool) -> Option<String> {
    let keyboard = keyboard(language)?;
    let mut text = String::new();
    for key in keys {
        text.push_str(keyboard.keys.get(key)?);
        if let Some((start, _)) = text.char_indices().rev().nth(1)
            && let Some(to) = keyboard.transforms.get(&text[start..])
        {
            text.truncate(start);
            text.push_str(to);
        }
    }
    if partial {
        // A dead key can be hidden by the host until its next key commits.
        for dead in &keyboard.dead {
            if text.ends_with(dead) {
                text.truncate(text.len() - dead.len());
                break;
            }
        }
    }
    (!text.chars().any(|ch| u32::from(ch) >= 0xf0000)).then_some(text)
}

/// Infers one round-tripping key sequence for manual candidates.
///
/// This result is never physical-key provenance. Dead-key pairs can represent
/// a character that has no single-key representation.
#[must_use]
pub fn infer(text: &str, language: Language) -> Option<Vec<char>> {
    let keyboard = keyboard(language)?;
    let mut reverse: Vec<(String, Vec<char>)> = keyboard
        .keys
        .iter()
        .filter(|(_, value)| !keyboard.dead.contains(value))
        .map(|(key, value)| (value.clone(), vec![*key]))
        .collect();
    for (from, to) in &keyboard.transforms {
        let mut sequence = Vec::new();
        for ch in from.chars() {
            let Some((key, _)) = keyboard
                .keys
                .iter()
                .find(|(_, value)| value.as_str() == ch.to_string())
            else {
                sequence.clear();
                break;
            };
            sequence.push(*key);
        }
        if !sequence.is_empty() {
            reverse.push((to.clone(), sequence));
        }
    }
    reverse.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.1.len().cmp(&b.1.len())));
    let mut remaining = text;
    let mut keys = Vec::new();
    while !remaining.is_empty() {
        let (prefix, sequence) = reverse
            .iter()
            .find(|(prefix, _)| remaining.starts_with(prefix))?;
        keys.extend(sequence);
        remaining = &remaining[prefix.len()..];
    }
    (render(&keys, language, false).as_deref() == Some(text)).then_some(keys)
}

/// Per-language validation used by both automatic and manual decisions.
pub trait AdditionalValidators {
    fn validate(&self, language: Language, candidate: &str) -> Validation;
}

/// Only selected dictionaries are loaded. Files are resolved from an explicit
/// application resource directory, never from user paths or the environment.
#[derive(Default)]
pub struct Dictionaries {
    loaded: BTreeMap<String, Dictionary>,
}

enum Dictionary {
    Hunspell(Box<spellbook::Dictionary>),
    ListedWords(BTreeSet<String>),
}

#[derive(Clone, Copy)]
enum FlagMode {
    Character,
    Long,
    Numeric,
}

impl FlagMode {
    fn parse(self, flags: &str) -> Option<Vec<String>> {
        match self {
            Self::Character => Some(flags.chars().map(|flag| flag.to_string()).collect()),
            Self::Long => {
                let chars: Vec<_> = flags.chars().collect();
                if !chars.len().is_multiple_of(2) {
                    return None;
                }
                Some(chars.chunks(2).map(|pair| pair.iter().collect()).collect())
            }
            Self::Numeric => flags
                .split(',')
                .map(|flag| flag.parse::<u16>().ok().map(|value| value.to_string()))
                .collect(),
        }
    }
}

fn listed_words(affixes: &str, entries: &str) -> BTreeSet<String> {
    let mode = affixes
        .lines()
        .find_map(|line| line.strip_prefix("FLAG "))
        .map_or(FlagMode::Character, |value| match value.trim() {
            "long" => FlagMode::Long,
            "num" => FlagMode::Numeric,
            _ => FlagMode::Character,
        });
    let mut forbidden_flags = BTreeSet::new();
    let mut restricted_flags = BTreeSet::new();
    for line in affixes.lines() {
        let mut fields = line.split_whitespace();
        let (Some(directive), Some(flags)) = (fields.next(), fields.next()) else {
            continue;
        };
        if matches!(
            directive,
            "FORBIDDENWORD" | "NEEDAFFIX" | "PSEUDOROOT" | "ONLYINCOMPOUND" | "FORCEUCASE"
        ) {
            let Some(flags) = mode.parse(flags) else {
                return BTreeSet::new();
            };
            if directive == "FORBIDDENWORD" {
                forbidden_flags.extend(flags.iter().cloned());
            }
            restricted_flags.extend(flags);
        }
    }
    let mut words = BTreeSet::new();
    let mut forbidden = BTreeSet::new();
    for line in entries.lines().skip(1) {
        let Some(entry) = line.split_whitespace().next() else {
            continue;
        };
        let (surface, flags) = entry.split_once('/').unwrap_or((entry, ""));
        if surface.is_empty() || surface.contains('\\') {
            continue;
        }
        let flags = if flags.is_empty() {
            Vec::new()
        } else {
            let Some(flags) = mode.parse(flags) else {
                continue;
            };
            flags
        };
        if flags.iter().any(|flag| forbidden_flags.contains(flag)) {
            forbidden.insert(surface.to_owned());
        }
        if !flags.iter().any(|flag| restricted_flags.contains(flag)) {
            words.insert(surface.to_owned());
        }
    }
    // A forbidden entry overrides another entry for the same surface. Never
    // accept affix-dependent or compound-only stems as standalone words.
    words.retain(|word| !forbidden.contains(word));
    words
}

impl Dictionaries {
    /// Loads enabled dictionaries before the input observer is activated.
    /// Missing or invalid resources remain unavailable rather than rejecting
    /// every word or falling back to another language.
    #[must_use]
    pub fn load(root: &Path, enabled: LanguageSet) -> Self {
        let mut loaded = BTreeMap::new();
        for language in enabled.enabled() {
            let Language::Additional(index) = language else {
                continue;
            };
            let info = &catalog().languages[index as usize];
            let result = std::fs::read_to_string(root.join(&info.dictionary).join("index.aff"))
                .and_then(|aff| {
                    let dic =
                        std::fs::read_to_string(root.join(&info.dictionary).join("index.dic"))?;
                    match info.validation {
                        DictionaryMode::Hunspell => spellbook::Dictionary::new(&aff, &dic)
                            .map(|dictionary| Dictionary::Hunspell(Box::new(dictionary)))
                            .map_err(|_| std::io::Error::other("dictionary-invalid")),
                        DictionaryMode::ListedWords => {
                            // The pinned exceptions have invalid affix/flag syntax. Do
                            // not invent inflections or treat an unlisted form as a
                            // rejection. Preserve all original sources for repair.
                            Ok(Dictionary::ListedWords(listed_words(&aff, &dic)))
                        }
                    }
                });
            if let Ok(dictionary) = result {
                loaded.insert(info.id.clone(), dictionary);
            } else {
                tracing::warn!(
                    language = info.id,
                    reason = "dictionary-unavailable",
                    "fixkey dictionary unavailable"
                );
            }
        }
        Self { loaded }
    }

    #[must_use]
    pub fn ready(&self, enabled: LanguageSet) -> bool {
        enabled.enabled().all(|language| {
            !matches!(language, Language::Additional(_))
                || self.loaded.contains_key(language.as_str())
        })
    }
}

impl AdditionalValidators for Dictionaries {
    fn validate(&self, language: Language, candidate: &str) -> Validation {
        self.loaded
            .get(language.as_str())
            .map_or(Validation::Unavailable, |dictionary| match dictionary {
                Dictionary::Hunspell(dictionary) => {
                    if dictionary.check(candidate) {
                        Validation::Accepted
                    } else {
                        Validation::Rejected
                    }
                }
                Dictionary::ListedWords(words) => {
                    if words.contains(candidate) {
                        Validation::Accepted
                    } else {
                        Validation::Unavailable
                    }
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_mode_retains_standalone_restrictions_and_forbidden_overrides() {
        let words = listed_words(
            "FLAG num\nFORBIDDENWORD 6\nNEEDAFFIX 8\nONLYINCOMPOUND 36\n",
            "6\nword/1,2\nstem/8\ncompound/36\nforbidden/6\nforbidden/1\nmalformed/17X\n",
        );
        assert_eq!(words, BTreeSet::from(["word".to_owned()]));
    }
}
