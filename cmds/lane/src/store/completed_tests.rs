// SPDX-License-Identifier: Apache-2.0
use super::*;
use rusqlite::params;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../protos/lane/v1/fixtures/completed-list.json"
    ))
    .unwrap()
}

#[test]
fn completed_entry_vectors() {
    let suite = fixture();
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE versions (entity_id TEXT, version_id TEXT, envelope_json TEXT);")
        .unwrap();
    let versions: HashMap<String, Value> = suite["dataset"]["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|version| {
            c.execute(
                "INSERT INTO versions VALUES (?,?,?)",
                params![
                    strv(version, "entityId"),
                    strv(version, "versionId"),
                    version.to_string()
                ],
            )
            .unwrap();
            (strv(version, "versionId").to_owned(), version.clone())
        })
        .collect();
    for case in suite["entryCases"].as_array().unwrap() {
        let input = &case["input"];
        let selected = &versions[strv(input, "selectedVersionId")];
        let heads: Vec<Value> = input["headVersionIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| versions[id.as_str().unwrap()].clone())
            .collect();
        let got = entry_for_heads(
            &c,
            strv(selected, "entityId"),
            &heads,
            strv(&selected["task"], "progress"),
        )
        .unwrap();
        if case["expected"]["kind"] == "TERMINAL_ENTRY_KIND_UNKNOWN" {
            assert!(got.is_none(), "{}", case["id"]);
        } else {
            let (_, time) = got.unwrap_or_else(|| panic!("{}", case["id"]));
            assert_eq!(
                time,
                timestamp(strv(&case["expected"], "enteredAt")).unwrap(),
                "{}",
                case["id"]
            );
        }
    }
}
