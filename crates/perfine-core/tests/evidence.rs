// SPDX-License-Identifier: Apache-2.0
mod common;
use common::*;
use perfine_core::{evidence::*, policy::*, types::*};

#[test]
fn rejects_non_v7_ids_unsupported_versions_and_invalid_shas() {
    for id in [
        "",
        "00000000-0000-0000-0000-000000000000",
        "550e8400-e29b-41d4-a716-446655440000",
        "018f2e8a-0000-7000-0000-000000000001",
    ] {
        assert!(ResourceId::new(id).is_err());
    }
    assert!(
        serde_json::from_str::<ResourceId>("\"550e8400-e29b-41d4-a716-446655440000\"").is_err()
    );
    assert!(serde_json::from_str::<Version>("2").is_err());
    assert!(CommitSha::new("main").is_err());
    assert!(CommitSha::new("0".repeat(40)).is_err());
    let id = ResourceId::generate();
    assert_eq!(
        serde_json::from_str::<ResourceId>(&serde_json::to_string(&id).unwrap()).unwrap(),
        id
    );
}
#[test]
fn required_configuration_fields_cannot_default_into_validity() {
    let value = serde_json::to_value(config()).unwrap();
    for key in [
        "limits",
        "policy",
        "conditions",
        "workload_confirmation",
        "initial_base",
        "schema_version",
    ] {
        let mut v = value.clone();
        v.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<Config>(v).is_err(), "{key}");
    }
    let mut v = value;
    v["future_additive_field"] = true.into();
    assert!(
        serde_json::from_value::<Config>(v)
            .unwrap()
            .freeze()
            .is_ok()
    );
}
#[test]
fn fingerprints_bind_every_material_condition_and_policy() {
    let cfg = config();
    let original = cfg.clone().freeze().unwrap();
    let mut mutations = vec![];
    let mut c = cfg.clone();
    c.initial_base = sha('e');
    mutations.push(c);
    let mut c = cfg.clone();
    c.incremental_base = sha('e');
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions
        .inputs
        .insert(name("input"), digest(b"changed"));
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions.workflow = sha('e');
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions.harness = sha('e');
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions
        .tools
        .insert(name("callgrind"), name("changed"));
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions.environment.runner = name("other");
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions.runtime_flags.push("--jitless".into());
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions.build_options.push("debug".into());
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions
        .cases
        .get_mut(&name("small"))
        .unwrap()
        .frequency = Some(2.0);
    mutations.push(c);
    let mut c = cfg.clone();
    c.conditions
        .cases
        .get_mut(&name("small"))
        .unwrap()
        .protected = true;
    mutations.push(c);
    let mut c = cfg;
    c.policy = Policy::MeasurementsOnly;
    mutations.push(c);
    for changed in mutations {
        assert_ne!(
            original.fingerprint(),
            changed.freeze().unwrap().fingerprint()
        );
    }
    let mut serialized = serde_json::to_value(&original).unwrap();
    serialized["config"]["initial_base"] = serde_json::to_value(sha('e')).unwrap();
    assert!(
        serde_json::from_value::<FrozenConfig>(serialized)
            .unwrap()
            .validate()
            .is_err()
    );
}
#[test]
fn malformed_or_incomplete_evidence_never_verifies() {
    let cfg = config().freeze().unwrap();
    let id = ResourceId::generate();
    let original = manifest(&cfg, &id, None, sha('a'), [100.0, 100.0]);
    let mut mutations = vec![];
    let mut m = original.clone();
    m.completion = Completion::Incomplete;
    mutations.push(m);
    let mut m = original.clone();
    m.measurements.remove(&name("small"));
    mutations.push(m);
    let mut m = original.clone();
    let renamed = m.measurements.remove(&name("small")).unwrap();
    m.measurements.insert(name("renamed"), renamed);
    mutations.push(m);
    let mut m = original.clone();
    m.conditions.runtime_flags.push("--jitless".into());
    mutations.push(m);
    let mut m = original.clone();
    m.conditions.metric.unit = name("nanoseconds");
    mutations.push(m);
    let mut m = original.clone();
    m.correctness.clear();
    mutations.push(m);
    let mut m = original.clone();
    m.profile_paths.get_mut(&name("small")).unwrap().clear();
    mutations.push(m);
    let mut m = original.clone();
    m.artifacts.remove(&name("profile"));
    mutations.push(m);
    let mut m = original.clone();
    m.artifacts.get_mut(&name("profile")).unwrap().sha256 = digest(b"other");
    mutations.push(m);
    for values in [
        vec![],
        vec![0.0],
        vec![-1.0],
        vec![f64::NAN],
        vec![f64::INFINITY],
        vec![100.5],
        vec![9_007_199_254_740_994.0],
    ] {
        let mut m = original.clone();
        m.measurements.get_mut(&name("small")).unwrap().values = values;
        mutations.push(m);
    }
    for m in mutations {
        assert!(
            verify(&cfg, &original.identity, m, &|n: &Name| Ok(n
                .as_str()
                .as_bytes()
                .to_vec()))
            .is_err()
        );
    }
    assert!(
        verify(&cfg, &original.identity, original.clone(), &|_: &Name| Err(
            Error::Incomplete("download")
        ))
        .is_err()
    );
}
#[test]
fn rejects_identity_substitution_even_with_valid_artifacts() {
    let f = Fixture::new(config(), [100.0, 100.0], [90.0, 90.0]);
    let mut m = f.head.manifest().clone();
    m.identity.source = sha('e');
    assert!(
        verify(&f.cfg, &f.head.manifest().identity, m, &|n: &Name| Ok(n
            .as_str()
            .as_bytes()
            .to_vec()))
        .is_err()
    );
}
#[test]
fn baseline_requires_two_distinct_same_sha_correct_runs() {
    let f = Fixture::new(config(), [100.0, 100.0], [90.0, 90.0]);
    assert!(validate_baseline(&f.cfg, &f.base, &f.base).is_err());
    assert!(validate_baseline(&f.cfg, &f.base, &f.head).is_err());
    let changed = run(&f.cfg, &f.experiment, None, sha('a'), [100.0, 101.0]);
    assert!(validate_baseline(&f.cfg, &f.base, &changed).is_err());
    let mut m = f.base.manifest().clone();
    m.identity.run = ResourceId::generate();
    m.correctness
        .insert(name("project-tests"), CheckResult::Failed);
    assert!(validate_baseline(&f.cfg, &f.base, &verified(&f.cfg, m)).is_err());
}

#[test]
fn local_evidence_round_trips_and_detects_corruption_without_overwriting_it() {
    use perfine_core::storage::EvidenceDirectory;
    let dir = tempfile::tempdir().unwrap();
    let store = EvidenceDirectory::open(dir.path()).unwrap();
    let f = Fixture::new(config(), [100.0, 100.0], [90.0, 90.0]);
    assert!(store.put_run(&f.head).is_err());
    for name in f.head.manifest().artifacts.keys() {
        store.put(name.as_str().as_bytes()).unwrap();
    }
    let hash = store.put_run(&f.head).unwrap();
    let recovered = store
        .load_run(&f.cfg, &f.head.manifest().identity, &hash)
        .unwrap();
    assert_eq!(recovered.digest(), f.head.digest());
    std::fs::write(dir.path().join(digest(b"profile").as_str()), b"tampered").unwrap();
    assert!(
        store
            .load_run(&f.cfg, &f.head.manifest().identity, &hash)
            .is_err()
    );
    assert!(store.put(b"profile").is_err());
}
#[test]
fn artifact_traversal_is_rejected_before_calling_the_reader() {
    let cfg = config().freeze().unwrap();
    let mut m = manifest(
        &cfg,
        &ResourceId::generate(),
        None,
        sha('a'),
        [100.0, 100.0],
    );
    let a = m.artifacts.remove(&name("measurements")).unwrap();
    m.artifacts.insert(name("../outside"), a);
    assert!(
        verify(&cfg, &m.identity.clone(), m, &|_: &Name| panic!(
            "must not resolve traversal"
        ))
        .is_err()
    );
}
