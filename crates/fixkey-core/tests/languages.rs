//! Offline language catalog and full-dictionary validation; no native access.

use std::path::Path;

use fixkey_core::{
    Language, LanguageSet,
    decision::Validation,
    multilingual::{AdditionalValidators, Dictionaries, catalog},
};

#[test]
fn real_dictionaries_correct_between_english_and_cyrillic_without_guessing_the_source() {
    use fixkey_core::{
        CorrectionOutcome,
        decision::{MissingValidator, Validators, decide},
        english::HunspellValidator,
    };
    let russian = Language::from_id("ru-xkb-ru").unwrap();
    let enabled = LanguageSet::default()
        .with(Language::KoDubeolsik, false)
        .with(russian, true);
    let dictionaries = Dictionaries::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/languages"),
        enabled,
    );
    let english = HunspellValidator::bundled().unwrap();
    let validators = Validators {
        additional: Some(&dictionaries),
        korean: &MissingValidator,
        english: &english,
    };
    for (word, keys, expected) in [("ghbdtn", "ghbdtn", "привет"), ("руддщ", "hello", "hello")]
    {
        let decision = decide(
            word,
            &keys.chars().collect::<Vec<_>>(),
            enabled,
            &validators,
        );
        assert_eq!(decision.outcome, CorrectionOutcome::Replaced, "{word}");
        assert_eq!(decision.replacement.as_deref(), Some(expected));
    }
    let mismatch = decide(
        "ghbdtn",
        &"other".chars().collect::<Vec<_>>(),
        enabled,
        &validators,
    );
    assert_eq!(mismatch.outcome, CorrectionOutcome::PreservedSkipped);
}

#[test]
fn malformed_upstream_rules_never_turn_unknown_forms_into_negative_evidence() {
    let danish = Language::from_id("da-xkb-dk").unwrap();
    let enabled = LanguageSet::default().with(danish, true);
    let dictionaries = Dictionaries::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/languages"),
        enabled,
    );
    assert_eq!(
        dictionaries.validate(danish, "tastatur"),
        Validation::Accepted
    );
    assert_eq!(
        dictionaries.validate(danish, "qzxjkvqzxjkv"),
        Validation::Unavailable
    );
}

#[test]
fn every_catalog_entry_has_a_unique_stable_identifier_and_a_loadable_dictionary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/languages");
    assert_eq!(Language::ALL.len(), catalog().languages.len() + 2);
    let mut failures = Vec::new();
    for language in Language::ALL.into_iter().skip(2) {
        assert_eq!(Language::from_id(language.as_str()), Some(language));
        let enabled = LanguageSet::default().with(language, true);
        let dictionaries = Dictionaries::load(&root, enabled);
        if !dictionaries.ready(enabled) {
            let info = &catalog()
                .languages
                .iter()
                .find(|info| info.id == language.as_str())
                .unwrap();
            let aff =
                std::fs::read_to_string(root.join(&info.dictionary).join("index.aff")).unwrap();
            let dic =
                std::fs::read_to_string(root.join(&info.dictionary).join("index.dic")).unwrap();
            let error = spellbook::Dictionary::new(&aff, &dic).err().unwrap();
            failures.push(format!("{language}: {error:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn representative_words_round_trip_under_their_published_keyboards() {
    for (id, word) in [
        ("de-xkb-de", "Tastatur"),
        ("fr-xkb-fr", "bonjour"),
        ("ru-xkb-ru", "привет"),
        ("uk-xkb-ua", "привіт"),
        ("el-xkb-gr", "καλημέρα"),
        ("he-xkb-il", "שלום"),
        ("es-xkb-es", "niño"),
        ("pl-xkb-pl", "język"),
        ("eo-xkb-epo", "ĉapelo"),
        ("tr-xkb-tr", "klavye"),
    ] {
        let language = Language::from_id(id).unwrap();
        let keys = fixkey_core::layout::keys_from_text(word, language).expect(word);
        assert_eq!(
            fixkey_core::layout::text_from_keys(&keys, language).as_deref(),
            Some(word),
            "{id}"
        );
        assert_eq!(fixkey_core::tokenizer::classify_token(word), None, "{id}");
    }
}

#[test]
fn missing_dictionary_is_unavailable_and_new_languages_are_opt_in() {
    let language = Language::from_id("ru-xkb-ru").unwrap();
    let enabled = LanguageSet::default().with(language, true);
    let dictionaries = Dictionaries::load(Path::new("missing-resources"), enabled);
    assert!(!dictionaries.ready(enabled));
    assert_eq!(
        dictionaries.validate(language, "привет"),
        Validation::Unavailable
    );
    assert!(!LanguageSet::default().is_enabled(language));
    assert_eq!(
        fixkey_core::tokenizer::classify_token("helloмир"),
        Some(fixkey_core::SkipReason::MixedScript)
    );
}

#[test]
fn national_punctuation_and_dead_keys_preserve_the_complete_physical_word() {
    for (physical, rendered, body, boundary) in [
        ("rPthrgo!", "계속해!", "rPthrgo", "! "),
        ("ghbdtn!!", "ghbdtn!!", "ghbdtn", "!! "),
        ("2cole", "école", "2cole", " "),
        ("caf'e", "café", "caf'e", " "),
    ] {
        let keys: Vec<_> = physical.chars().collect();
        let (word_keys, actual_boundary) =
            fixkey_core::observation::word_at_space(&keys, rendered).unwrap();
        assert_eq!(word_keys.iter().collect::<String>(), body);
        assert_eq!(actual_boundary, boundary);
    }
    for text in [
        "https://example.com!",
        "user@example.com",
        "hello!world",
        "helloмир",
        "123",
    ] {
        assert!(
            fixkey_core::observation::word_at_space(&text.chars().collect::<Vec<_>>(), text)
                .is_none(),
            "{text}"
        );
    }
}
