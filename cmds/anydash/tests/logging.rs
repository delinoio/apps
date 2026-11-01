// SPDX-License-Identifier: Apache-2.0
//! Log rotation: daily files kept for 7 days or 50 MiB in total.
use anydash::logging::{self, MAX_TOTAL_BYTES};
use chrono::NaiveDate;
use std::io::Write as _;
const fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, d).expect("valid date")
}
#[test]
fn files_older_than_seven_days_are_deleted() {
    let dir = tempfile::tempdir().unwrap();
    for d in 20..=31 {
        std::fs::write(dir.path().join(logging::file_name(day(d))), b"x").unwrap();
    }
    std::fs::write(dir.path().join("unrelated.txt"), b"keep").unwrap();
    logging::prune(dir.path(), day(31), None).unwrap();
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.first().unwrap(), "anydash.2026-10-25.log");
    assert_eq!(names.len(), 8, "{names:?}");
    assert!(names.contains(&"unrelated.txt".to_owned()));
}
#[test]
fn total_size_is_capped_at_50_mib_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    for d in 28..=31 {
        let f = std::fs::File::create(dir.path().join(logging::file_name(day(d)))).unwrap();
        f.set_len(20 << 20).unwrap();
    }
    let current = dir.path().join(logging::file_name(day(31)));
    logging::prune(dir.path(), day(31), Some(&current)).unwrap();
    let total: u64 = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(total <= MAX_TOTAL_BYTES);
    assert!(!dir.path().join(logging::file_name(day(28))).exists());
    assert!(!dir.path().join(logging::file_name(day(29))).exists());
    assert!(current.exists());
    // A current file that alone exceeds the cap restarts empty.
    std::fs::File::create(&current)
        .unwrap()
        .set_len(60 << 20)
        .unwrap();
    logging::prune(dir.path(), day(31), Some(&current)).unwrap();
    assert!(std::fs::metadata(&current).unwrap().len() <= MAX_TOTAL_BYTES);
}
#[test]
fn rolling_writer_uses_daily_file_names() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = logging::Rolling::new(dir.path().join("logs"));
    writeln!(writer, "event=test").unwrap();
    writer.flush().unwrap();
    let name = logging::file_name(chrono::Utc::now().date_naive());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("logs").join(name)).unwrap(),
        "event=test\n"
    );
}
