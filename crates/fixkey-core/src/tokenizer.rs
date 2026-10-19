//! Word boundary detection and eligibility classification for typed text.
//!
//! The tokenizer only sees characters that the input-event adapter reports as
//! directly typed after IME composition completed. It holds characters whose
//! role (internal word character or trailing punctuation) is decided by the
//! next input, and it never triggers evaluation on Enter or Tab.

use crate::types::SkipReason;

/// Characters that can be internal to a word: apostrophes and hyphens.
const JOINERS: [char; 3] = ['\'', '\u{2019}', '-'];
/// Separators that can occur inside URLs and email addresses.
const URL_SEPARATORS: [char; 6] = ['.', '/', ':', '@', '?', '#'];

fn is_joiner(c: char) -> bool {
    JOINERS.contains(&c)
}

/// Includes combining marks and script-internal joiners in one word.
#[must_use]
pub fn is_word_character(character: char) -> bool {
    character.is_alphabetic()
        || is_joiner(character)
        || matches!(u32::from(character),
        0x0300..=0x036f | 0x0591..=0x05bd | 0x05bf | 0x05c1..=0x05c2 | 0x05c4..=0x05c5 |
        0x0610..=0x061a | 0x064b..=0x065f | 0x0670 | 0x06d6..=0x06ed |
        0x0900..=0x0903 | 0x093a..=0x094f | 0x0951..=0x0957 | 0x0962..=0x0963 | 0x200c)
}

fn is_held_separator(c: char) -> bool {
    is_joiner(c) || c == '_' || URL_SEPARATORS.contains(&c)
}

fn is_emoji(c: char) -> bool {
    matches!(
        u32::from(c),
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xFE0F | 0x200D | 0x20E3
    )
}

fn is_hangul(c: char) -> bool {
    matches!(u32::from(c), 0xAC00..=0xD7A3 | 0x3131..=0x3163)
}

/// Result of finishing a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenOutcome {
    /// A word is complete and eligible for evaluation.
    Evaluate {
        /// Completed word without its boundary.
        word: String,
        /// Boundary characters that followed the word, preserved on
        /// replacement.
        boundary: String,
    },
    /// The token is excluded from automatic correction.
    Skipped(SkipReason),
}

/// Classifies a pending token, returning the exclusion reason if any.
///
/// A token that survives contains only letters of one supported script plus
/// internal apostrophes and hyphens.
#[must_use]
pub fn classify_token(token: &str) -> Option<SkipReason> {
    if token.chars().any(is_emoji) {
        return Some(SkipReason::Emoji);
    }
    if token.contains('@') && !token.starts_with('@') && !token.ends_with('@') {
        return Some(SkipReason::Email);
    }
    if token
        .chars()
        .any(|c| URL_SEPARATORS.contains(&c) || c == '_')
    {
        return Some(SkipReason::Url);
    }
    if token.chars().any(char::is_numeric) {
        return Some(SkipReason::Numeric);
    }
    let mut previous_script = None;
    for ch in token.chars() {
        let code = u32::from(ch);
        let script = match code {
            0x0041..=0x024f | 0x1e00..=0x1eff if ch.is_alphabetic() => Some(1),
            0x0370..=0x03ff | 0x1f00..=0x1fff => Some(2),
            0x0400..=0x052f => Some(3),
            0x0530..=0x058f => Some(4),
            0x0590..=0x05ff => Some(5),
            0x0600..=0x06ff | 0x0750..=0x077f | 0x08a0..=0x08ff => Some(6),
            0x0900..=0x097f => Some(7),
            0x10a0..=0x10ff | 0x2d00..=0x2d2f | 0x1c90..=0x1cbf => Some(8),
            _ if is_hangul(ch) => Some(9),
            0x0300..=0x036f => previous_script,
            _ if ch.is_alphabetic() => return Some(SkipReason::MixedScript),
            _ => None,
        };
        if let Some(script) = script {
            if previous_script.is_some_and(|previous| previous != script) {
                return Some(SkipReason::MixedScript);
            }
            previous_script = Some(script);
        }
    }
    None
}

/// Incremental tokenizer for one active correction context.
#[derive(Debug, Default)]
pub struct Tokenizer {
    token: String,
    held: String,
}

impl Tokenizer {
    /// Creates an empty tokenizer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether a token is pending.
    #[must_use]
    pub const fn has_pending(&self) -> bool {
        !self.token.is_empty()
    }

    /// Discards the pending token and any held separator.
    pub fn reset(&mut self) {
        self.token.clear();
        self.held.clear();
    }

    /// Feeds one directly typed character.
    pub fn feed_char(&mut self, c: char) -> Option<TokenOutcome> {
        if c.is_whitespace() {
            return self.finish(c);
        }
        if is_held_separator(c) {
            if self.token.is_empty() {
                // A separator with no word before it cannot start a word.
                return None;
            }
            self.held.push(c);
            return None;
        }
        if c.is_alphanumeric() || is_emoji(c) || is_word_character(c) {
            if !self.held.is_empty() {
                let held = std::mem::take(&mut self.held);
                self.token.push_str(&held);
            }
            self.token.push(c);
            return None;
        }
        // Any other punctuation ends the word.
        self.finish(c)
    }

    /// Feeds Enter or Tab.
    ///
    /// These keys never trigger evaluation by themselves. When a separator is
    /// held after a word, the word is classified once as trailing punctuation
    /// so the caller can evaluate it against the still-matching context;
    /// otherwise the pending token is discarded.
    pub fn feed_enter_or_tab(&mut self) -> Option<TokenOutcome> {
        if self.held.is_empty() {
            self.reset();
            return None;
        }
        self.finish_with_boundary("")
    }

    fn finish(&mut self, terminator: char) -> Option<TokenOutcome> {
        self.finish_with_boundary(terminator.encode_utf8(&mut [0; 4]))
    }

    fn finish_with_boundary(&mut self, terminator: &str) -> Option<TokenOutcome> {
        if self.token.is_empty() {
            self.reset();
            return None;
        }
        let word = std::mem::take(&mut self.token);
        let mut boundary = std::mem::take(&mut self.held);
        boundary.push_str(terminator);
        if let Some(reason) = classify_token(&word) {
            return Some(TokenOutcome::Skipped(reason));
        }
        Some(TokenOutcome::Evaluate { word, boundary })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(tokenizer: &mut Tokenizer, text: &str) -> Vec<TokenOutcome> {
        text.chars()
            .filter_map(|c| tokenizer.feed_char(c))
            .collect()
    }

    fn evaluate(word: &str, boundary: &str) -> TokenOutcome {
        TokenOutcome::Evaluate {
            word: word.into(),
            boundary: boundary.into(),
        }
    }

    #[test]
    fn a_space_completes_a_word_and_is_preserved() {
        let mut t = Tokenizer::new();
        assert_eq!(feed(&mut t, "rPthrgo "), vec![evaluate("rPthrgo", " ")]);
        assert!(!t.has_pending());
    }

    #[test]
    fn punctuation_completes_a_word() {
        let mut t = Tokenizer::new();
        assert_eq!(feed(&mut t, "zlqhemrk,"), vec![evaluate("zlqhemrk", ",")]);
        assert_eq!(feed(&mut t, "abc!"), vec![evaluate("abc", "!")]);
    }

    #[test]
    fn internal_apostrophes_and_hyphens_stay_in_the_word() {
        let mut t = Tokenizer::new();
        assert_eq!(feed(&mut t, "don't "), vec![evaluate("don't", " ")]);
        assert_eq!(
            feed(&mut t, "well-known "),
            vec![evaluate("well-known", " ")]
        );
    }

    #[test]
    fn a_trailing_joiner_is_decided_by_the_next_character() {
        let mut t = Tokenizer::new();
        // Held: nothing is emitted while the decision is pending.
        assert_eq!(feed(&mut t, "dogs'"), vec![]);
        assert!(t.has_pending());
        assert_eq!(t.feed_char(' '), Some(evaluate("dogs", "' ")));
        assert_eq!(feed(&mut t, "go- "), vec![evaluate("go", "- ")]);
    }

    #[test]
    fn enter_and_tab_never_trigger_evaluation() {
        let mut t = Tokenizer::new();
        assert_eq!(feed(&mut t, "abc"), vec![]);
        assert_eq!(t.feed_enter_or_tab(), None);
        assert!(!t.has_pending());
        feed(&mut t, "abc");
        assert_eq!(t.feed_enter_or_tab(), None);
    }

    #[test]
    fn a_held_separator_before_enter_evaluates_once_as_trailing_punctuation() {
        let mut t = Tokenizer::new();
        feed(&mut t, "rPthrgo.");
        assert_eq!(t.feed_enter_or_tab(), Some(evaluate("rPthrgo", ".")));
        assert!(!t.has_pending());
    }

    #[test]
    fn a_held_separator_before_enter_is_discarded_for_excluded_tokens() {
        let mut t = Tokenizer::new();
        feed(&mut t, "123.");
        assert_eq!(
            t.feed_enter_or_tab(),
            Some(TokenOutcome::Skipped(SkipReason::Numeric))
        );
    }

    #[test]
    fn excluded_tokens_are_skipped_with_a_reason() {
        let mut t = Tokenizer::new();
        let cases = [
            ("abc123 ", SkipReason::Numeric),
            ("42 ", SkipReason::Numeric),
            ("hi😀 ", SkipReason::Emoji),
            ("https://example.com/a ", SkipReason::Url),
            ("example.com ", SkipReason::Url),
            ("snake_case ", SkipReason::Url),
            ("me@example.com ", SkipReason::Email),
            ("a가 ", SkipReason::MixedScript),
            ("你好 ", SkipReason::MixedScript),
        ];
        for (input, reason) in cases {
            assert_eq!(
                feed(&mut t, input),
                vec![TokenOutcome::Skipped(reason)],
                "{input}"
            );
        }
    }

    #[test]
    fn separators_do_not_trigger_evaluation_on_their_own() {
        let mut t = Tokenizer::new();
        assert_eq!(feed(&mut t, "www."), vec![]);
        assert_eq!(feed(&mut t, "site.org"), vec![]);
        assert_eq!(
            t.feed_char(' '),
            Some(TokenOutcome::Skipped(SkipReason::Url))
        );
    }

    #[test]
    fn leading_separators_and_empty_tokens_produce_nothing() {
        let mut t = Tokenizer::new();
        assert_eq!(feed(&mut t, "  -- ' "), vec![]);
        assert!(!t.has_pending());
    }

    #[test]
    fn reset_clears_pending_state() {
        let mut t = Tokenizer::new();
        feed(&mut t, "abc-");
        t.reset();
        assert!(!t.has_pending());
        assert_eq!(feed(&mut t, "x "), vec![evaluate("x", " ")]);
    }

    #[test]
    fn hangul_words_are_eligible() {
        let mut t = Tokenizer::new();
        assert_eq!(feed(&mut t, "키보드가 "), vec![evaluate("키보드가", " ")]);
    }
}
