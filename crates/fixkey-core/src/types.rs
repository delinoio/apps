//! Stable identifiers shared by the engine, the desktop shell and diagnostics.
//!
//! The string forms returned by `as_str` are part of the public contract and
//! must not change without updating `docs/crates-fixkey-core-foundation.md`.

use std::fmt;

/// A supported language together with its keyboard layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Language {
    /// Korean Dubeolsik layout.
    KoDubeolsik,
    /// US English QWERTY layout.
    EnUsQwerty,
    /// An additional bundled dictionary and keyboard.
    Additional(crate::languages::AdditionalLanguage),
}

impl Language {
    /// Stable machine-readable identifier used in settings and diagnostics.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KoDubeolsik => "ko-dubeolsik",
            Self::EnUsQwerty => "en-us-qwerty",
            Self::Additional(language) => {
                &crate::multilingual::catalog().languages[language as usize].id
            }
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Selected languages. The original pair is enabled; additional languages are opt-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageSet {
    bits: u128,
}

impl Default for LanguageSet {
    fn default() -> Self {
        Self { bits: 3 }
    }
}

impl Language {
    const fn index(self) -> u32 {
        match self {
            Self::KoDubeolsik => 0,
            Self::EnUsQwerty => 1,
            Self::Additional(language) => language as u32 + 2,
        }
    }

    /// Parses a stable settings identifier. Unknown identifiers are rejected.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|language| language.as_str() == id)
    }
}

impl LanguageSet {
    /// Returns whether a public keyboard beyond the original pair is enabled.
    #[must_use]
    pub const fn has_additional(self) -> bool {
        self.bits & !3 != 0
    }

    /// Returns whether `language` is enabled.
    #[must_use]
    pub const fn is_enabled(self, language: Language) -> bool {
        self.bits & (1 << language.index()) != 0
    }

    /// Returns a copy with `language` enabled or disabled.
    #[must_use]
    pub const fn with(mut self, language: Language, enabled: bool) -> Self {
        if enabled {
            self.bits |= 1 << language.index();
        } else {
            self.bits &= !(1 << language.index());
        }
        self
    }

    /// Enabled languages in stable order. New languages are opt-in.
    pub fn enabled(self) -> impl Iterator<Item = Language> {
        Language::ALL
            .into_iter()
            .filter(move |language| self.is_enabled(*language))
    }
}

/// Result of evaluating one completed word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorrectionOutcome {
    /// The word was replaced by the single accepted candidate.
    Replaced,
    /// No enabled language accepted its candidate.
    PreservedNoMatch,
    /// More than one enabled language accepted its candidate.
    PreservedAmbiguous,
    /// The only accepted candidate equals the existing word.
    PreservedUnchanged,
    /// The word was not eligible; see [`SkipReason`].
    PreservedSkipped,
    /// A required enabled-language validator was unavailable or failed.
    PreservedValidatorUnavailable,
    /// The captured context no longer matched the target.
    PreservedStaleContext,
}

impl CorrectionOutcome {
    /// Stable identifier used in diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Replaced => "replaced",
            Self::PreservedNoMatch => "preserved-no-match",
            Self::PreservedAmbiguous => "preserved-ambiguous",
            Self::PreservedUnchanged => "preserved-unchanged",
            Self::PreservedSkipped => "preserved-skipped",
            Self::PreservedValidatorUnavailable => "preserved-validator-unavailable",
            Self::PreservedStaleContext => "preserved-stale-context",
        }
    }
}

/// Why a word or key event was not read or corrected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// IME composition has not completed.
    Composing,
    /// The text was pasted.
    Pasted,
    /// The text was inserted programmatically or its provenance is unknown.
    Programmatic,
    /// The token contains digits.
    Numeric,
    /// The token contains emoji.
    Emoji,
    /// The token is a URL.
    Url,
    /// The token is an email address.
    Email,
    /// The token mixes scripts.
    MixedScript,
    /// The focused field is a password or protected field.
    ProtectedField,
    /// The focused field could not be classified.
    UnknownField,
    /// A required permission is missing.
    PermissionMissing,
    /// The target does not support safe replacement.
    UnsupportedWrite,
    /// The focused application is excluded by the user.
    ExcludedApp,
    /// Correction is paused.
    Paused,
    /// The text is `FixKey`'s own replacement.
    SelfReplacement,
    /// The word was just restored by undo.
    RestoredWord,
}

impl SkipReason {
    /// Stable identifier used in diagnostics and application status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Composing => "composing",
            Self::Pasted => "pasted",
            Self::Programmatic => "programmatic",
            Self::Numeric => "numeric",
            Self::Emoji => "emoji",
            Self::Url => "url",
            Self::Email => "email",
            Self::MixedScript => "mixed-script",
            Self::ProtectedField => "protected-field",
            Self::UnknownField => "unknown-field",
            Self::PermissionMissing => "permission-missing",
            Self::UnsupportedWrite => "unsupported-write",
            Self::ExcludedApp => "excluded-app",
            Self::Paused => "paused",
            Self::SelfReplacement => "self-replacement",
            Self::RestoredWord => "restored-word",
        }
    }
}

/// Validation state of a manual candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateValidation {
    /// The dictionary recognizes the candidate.
    Recognized,
    /// The candidate is layout-convertible but not recognized.
    NotRecognized,
    /// The validator could not check the candidate.
    ValidatorUnavailable,
}

impl CandidateValidation {
    /// Stable identifier used in diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recognized => "recognized",
            Self::NotRecognized => "not-recognized",
            Self::ValidatorUnavailable => "validator-unavailable",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_are_enabled_by_default() {
        let set = LanguageSet::default();
        assert_eq!(
            set.enabled().collect::<Vec<_>>(),
            [Language::KoDubeolsik, Language::EnUsQwerty]
        );
    }

    #[test]
    fn languages_can_be_disabled_independently() {
        let set = LanguageSet::default().with(Language::KoDubeolsik, false);
        assert!(!set.is_enabled(Language::KoDubeolsik));
        assert!(set.is_enabled(Language::EnUsQwerty));
        assert_eq!(Language::KoDubeolsik.to_string(), "ko-dubeolsik");
    }

    #[test]
    fn identifiers_match_the_contract() {
        assert_eq!(
            CorrectionOutcome::PreservedValidatorUnavailable.as_str(),
            "preserved-validator-unavailable"
        );
        assert_eq!(SkipReason::MixedScript.as_str(), "mixed-script");
        assert_eq!(
            CandidateValidation::NotRecognized.as_str(),
            "not-recognized"
        );
    }
}
