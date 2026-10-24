// SPDX-License-Identifier: FSL-1.1-ALv2
use perfine_api::evaluation::evidence::FrozenConfig;

#[test]
fn client_v1_configuration_preserves_canonical_fingerprint() {
    let frozen: FrozenConfig =
        serde_json::from_str(include_str!("fixtures/client-v1-config.json")).unwrap();
    frozen.validate().unwrap();
    let rebuilt = frozen.config().clone().freeze().unwrap();
    assert_eq!(rebuilt.fingerprint(), frozen.fingerprint());
    let mut changed = frozen.config().clone();
    changed.limits.candidate_count += 1;
    assert_ne!(
        changed.freeze().unwrap().fingerprint(),
        frozen.fingerprint()
    );
}
