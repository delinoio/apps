// SPDX-License-Identifier: FSL-1.1-ALv2
use perfine_api::{evidence::content_allowed, hash};
use std::io::Write;
const RAW: &str = "ARTIFACT_KIND_RAW_PROFILE";
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
fn varint(mut n: usize) -> Vec<u8> {
    let mut b = Vec::new();
    while n > 127 {
        b.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    b.push(n as u8);
    b
}
fn field(tag: u8, bytes: &[u8]) -> Vec<u8> {
    let mut b = vec![tag << 3 | 2];
    b.extend(varint(bytes.len()));
    b.extend(bytes);
    b
}
fn profile(strings: &[&str]) -> Vec<u8> {
    let mut b = field(6, b"");
    for s in strings {
        b.extend(field(6, s.as_bytes()));
    }
    b
}
#[test]
fn six_language_raw_evidence_preserves_bytes() {
    for source in ["hot.rs", "hot.c", "hot.cpp", "hot.zig"] {
        let text = format!("version: 1\nevents: Ir\nfl={source}\nfn=hot\n1 10\n");
        assert!(content_allowed(text.as_bytes(), RAW).is_ok());
    }
    let node = br#"{"nodes":[{"id":1,"callFrame":{"functionName":"hot","url":"hot.js","scriptId":"1","lineNumber":1,"columnNumber":0},"hitCount":1}],"samples":[1]}"#;
    assert!(content_allowed(node, RAW).is_ok());
    let original = gzip(&profile(&["hot.go", "cpu", "nanoseconds"]));
    let before = hash(&original);
    assert!(content_allowed(&original, RAW).is_ok());
    assert_eq!(before, hash(&original));
    assert!(content_allowed(&original, "ARTIFACT_KIND_PROFILE_SUMMARY").is_err());
    assert!(content_allowed(&original, "ARTIFACT_KIND_RAW_TEST_LOG").is_err());
}
#[test]
fn pprof_scans_unused_strings_and_every_source_metadata_reference() {
    // All pprof text is in the string table; scan even unreachable or overwritten entries.
    for value in [
        "ghp_fixture",
        "source.go\npassword=fixture",
        "Authorization: Bearer fixture",
        "client_secret=fixture",
    ] {
        assert!(content_allowed(&gzip(&profile(&[value])), RAW).is_err());
    }
    // Function system name/filename, mapping filename/build ID, labels, comments, doc URL.
    for bytes in [
        field(5, &[24, 1, 32, 1]),
        field(3, &[40, 1, 48, 1]),
        field(2, &field(3, &[8, 1, 16, 1, 32, 1])),
        vec![104, 1],
        vec![120, 1],
    ] {
        let mut p = profile(&["source snippet"]);
        p.extend(&bytes);
        assert!(content_allowed(&gzip(&p), RAW).is_ok());
        let mut p = profile(&["access_token=fixture"]);
        p.extend(&bytes);
        assert!(content_allowed(&gzip(&p), RAW).is_err());
    }
}
#[test]
fn rejects_unknown_binary_unknown_fields_invalid_wire_and_compression_smuggling() {
    for raw in [vec![0, 1, 2], vec![0xff, 0xfe], vec![0x08, 0x01]] {
        assert!(content_allowed(&raw, RAW).is_err());
    }
    let valid = profile(&["hot.go"]);
    for suffix in [
        vec![0x82, 1, 1, b'x'],
        field(5, &[0x32, 1, b'x']),
        vec![0x38, 99],
        vec![0x30, 1],
        vec![0x32, 0xff],
        vec![0xff; 11],
    ] {
        let mut p = valid.clone();
        p.extend(suffix);
        assert!(content_allowed(&gzip(&p), RAW).is_err());
    }
    assert!(content_allowed(&gzip(b"not protobuf"), RAW).is_err());
    let mut packed = valid.clone();
    packed.extend([106, 2, 0, 1]);
    assert!(content_allowed(&gzip(&packed), RAW).is_ok());
    for suffix in [b"trailing".to_vec(), gzip(&valid)] {
        let mut p = gzip(&valid);
        p.extend(suffix);
        assert!(content_allowed(&p, RAW).is_err());
    }
    let mut p = gzip(&valid);
    p.pop();
    assert!(content_allowed(&p, RAW).is_err());
    let mut p = gzip(&valid);
    let end = p.len();
    p[end - 8] ^= 1;
    assert!(content_allowed(&p, RAW).is_err());
    let mut encoder = flate2::GzBuilder::new()
        .comment("password=fixture")
        .write(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&valid).unwrap();
    assert!(content_allowed(&encoder.finish().unwrap(), RAW).is_err());
    let mut encoder = flate2::GzBuilder::new()
        .extra(b"unknown".to_vec())
        .write(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&valid).unwrap();
    assert!(content_allowed(&encoder.finish().unwrap(), RAW).is_err());
}
#[test]
fn bounded_decompression_rejects_bombs() {
    let p = gzip(&vec![0; (64 << 20) + 1]);
    assert!(p.len() < 1 << 20);
    assert!(content_allowed(&p, RAW).is_err());
}
#[test]
fn json_escapes_and_duplicate_keys_do_not_hide_secrets_or_source() {
    for bytes in [
        br#"{"name":"ghp\u005ffixture"}"#.as_slice(),
        br#"{"name":"ghp\u005ffixture","name":"safe"}"#,
        br#"{"api\u005fkey":"fixture"}"#,
    ] {
        assert!(content_allowed(bytes, RAW).is_err());
        assert!(content_allowed(bytes, "ARTIFACT_KIND_PROFILE_SUMMARY").is_err());
    }
    assert!(
        content_allowed(
            br#"{"source\u005fcode":"fn hot() {}"}"#,
            "ARTIFACT_KIND_PROFILE_SUMMARY"
        )
        .is_err()
    );
    assert!(content_allowed(br#"{"source\u005fcode":"fn hot() {}"}"#, RAW).is_ok());
}

#[test]
fn captured_native_profiles_are_eligible_without_rewriting() {
    for bytes in [
        include_bytes!("fixtures/go-darwin-arm64.pprof").as_slice(),
        include_bytes!("fixtures/node-darwin-arm64.cpuprofile").as_slice(),
        include_bytes!("fixtures/c-linux-amd64-emulated.callgrind").as_slice(),
    ] {
        assert!(content_allowed(bytes, RAW).is_ok());
    }
}
