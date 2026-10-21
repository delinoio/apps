// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("delcore-catalog-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn put(&self, name: &str, body: &str) {
        let p = self.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
const SERVICE: &str = "kind: service\nkey: api\nname: API\nclient_id: api\ncapabilities: [execute]\nscopes: [authorize]\n";
const PLAN: &str = "kind: plan\nkey: free\nname: Free\nrank: 0\nfree: true\ndefault_free: true\n";
#[test]
fn load_catalog_file_set() {
    let d = Directory::new();
    assert!(load(&d.0).is_err());
    assert!(load(&d.0.join("missing")).is_err());
    d.put("nested/service.yml", SERVICE);
    d.put("plan.yaml", PLAN);
    assert_eq!(load(&d.0).unwrap().len(), 2);
    for body in [
        SERVICE.to_owned(),
        format!("{PLAN}unknown: 1\n"),
        format!("{PLAN}features: [{{service: api, key: access, enabled: true, surprise: true}}]\n"),
        format!("{PLAN}features: [{{service: api, service: api, key: access, enabled: true}}]\n"),
        PLAN.replace("rank: 0", "rank: invalid"),
        format!("{PLAN}---\n{PLAN}"),
        "kind: [".into(),
    ] {
        d.put("invalid.yaml", &body);
        assert!(load(&d.0).is_err(), "accepted {body}");
        std::fs::remove_file(d.0.join("invalid.yaml")).unwrap();
    }
}
#[test]
fn count_meter_rejects_value_attribute() {
    let d = Directory::new();
    let head = "kind: meter\nservice: api\nkey: requests\nevent_type: request\n";
    d.put(
        "meter.yaml",
        &format!("{head}aggregation: count\nvalue_attribute: bytes\n"),
    );
    assert!(
        format!("{:#}", load(&d.0).unwrap_err())
            .contains("count aggregation cannot have a value attribute")
    );
    d.put("meter.yaml", &format!("{head}aggregation: count\n"));
    assert!(load(&d.0).is_ok());
    d.put(
        "meter.yaml",
        &format!("{head}aggregation: sum\nvalue_attribute: bytes\n"),
    );
    assert!(load(&d.0).is_ok());
}
#[test]
fn filter_literals_preserve_types_and_spelling() {
    for (operator, values, valid) in [
        ("equal", "[\"success\"]", true),
        ("equal", "[5]", true),
        ("not_equal", "[true]", true),
        ("in", "[\"a\", 2.5, false]", true),
        ("equal", "[2.50]", false),
        ("in", "[1e3]", false),
        ("equal", "[null]", false),
        ("equal", "[[1]]", false),
        ("greater_than", "[\"2.5\"]", true),
        ("greater_than", "[2.5]", true),
        ("greater_than", "[\"02.50\"]", false),
        ("less_than", "[true]", false),
        ("less_than", "[\"high\"]", false),
        ("exists", "[]", true),
        ("exists", "[\"present\"]", false),
        ("equal", "[01]", false),
        ("equal", "[0x10]", false),
        ("equal", "[.nan]", false),
        ("equal", "[-0]", false),
    ] {
        let d = Directory::new();
        d.put("meter.yaml",&format!("kind: meter\nkey: requests\nservice: api\nevent_type: request\naggregation: count\nfilters:\n  - attribute: value\n    operator: {operator}\n    values: {values}\n"));
        assert_eq!(load(&d.0).is_ok(), valid, "{operator} {values}");
    }
}
#[test]
fn normalize_feature_grant_types() {
    for (body, kind, vt, expected) in [
        ("enabled: true", 1, 0, Some(json!({"enabled":true}))),
        ("limit: \"10.00\"", 2, 0, Some(json!({"limit":"10"}))),
        ("configuration: false", 3, 1, Some(json!({"boolean":false}))),
        ("configuration: 42", 3, 2, Some(json!({"integer":42}))),
        (
            "configuration: \"1.50\"",
            3,
            3,
            Some(json!({"decimal":"1.5"})),
        ),
        (
            "configuration: \"eu-west\"",
            3,
            4,
            Some(json!({"text":"eu-west"})),
        ),
        (
            "configuration: [\"eu\", \"us\"]",
            3,
            5,
            Some(json!({"string_list":["eu","us"]})),
        ),
        ("configuration: \"true\"", 3, 1, None),
        ("configuration: 3.5", 3, 2, None),
        ("configuration: 1.5", 3, 3, None),
        ("limit: \"-1\"", 2, 0, None),
        ("configuration: [\"eu\", 3]", 3, 5, None),
    ] {
        let g = grant(&yaml::parse(body).unwrap()).unwrap();
        let actual = normalize_grant(&g, kind, vt);
        if let Some(value) = expected {
            assert_eq!(actual.unwrap(), value, "{body}");
        } else {
            assert!(actual.is_err(), "{body}");
        }
    }
}
#[test]
fn committed_example_covers_documented_format() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog");
    let items = load(&root).unwrap();
    let mut has = BTreeSet::new();
    let mut plan_features = BTreeSet::new();
    let mut addon_features = vec![];
    let mut prices: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for d in items {
        has.insert(format!(
            "ext:{}",
            d.path.extension().unwrap().to_str().unwrap()
        ));
        if d.path.strip_prefix(&root).unwrap().components().count() > 1 {
            has.insert("nested".into());
        }
        has.insert(format!("kind:{}", d.kind));
        match d.kind.as_str() {
            "meter" => {
                has.insert(format!("aggregation:{}", d.aggregation));
                if d.aggregation == "sum" && !d.filters.is_empty() && !d.group_by.is_empty() {
                    has.insert("sum-with-filters-and-group-by".into());
                }
                for f in d.filters {
                    has.insert(format!("operator:{}", f.operator));
                }
            }
            "feature" => {
                has.insert(format!("feature:{}:{}", d.feature_type, d.value_type));
            }
            "plan" => {
                for g in d.features {
                    plan_features.insert((g.service, g.key));
                }
            }
            "addon" => {
                for g in d.features {
                    addon_features.push((g.service, g.key));
                }
            }
            "price" => {
                let owner = d
                    .add_on
                    .map(|a| format!("addon:{}", a.key))
                    .unwrap_or(d.plan);
                let kinds = prices.entry(owner).or_default();
                kinds.insert(d.price_kind.clone());
                if d.price_kind == "usage" && d.included != "0" {
                    kinds.insert("included".into());
                }
            }
            _ => {}
        }
    }
    for want in [
        "ext:yaml",
        "ext:yml",
        "nested",
        "kind:service",
        "kind:meter",
        "kind:feature",
        "kind:plan",
        "kind:addon",
        "kind:price",
        "aggregation:count",
        "aggregation:sum",
        "aggregation:unique_count",
        "sum-with-filters-and-group-by",
        "operator:exists",
        "operator:in",
        "operator:not_equal",
        "operator:less_than",
        "feature:access:",
        "feature:metered:",
        "feature:configuration:boolean",
        "feature:configuration:integer",
        "feature:configuration:decimal",
        "feature:configuration:string",
        "feature:configuration:string_list",
    ] {
        assert!(has.contains(want), "sample catalog lacks {want}");
    }
    assert!(
        prices
            .values()
            .any(|p| p.contains("recurring") && p.contains("usage") && p.contains("included"))
    );
    assert!(!addon_features.is_empty());
    for f in addon_features {
        assert!(!plan_features.contains(&f));
    }
}

#[test]
fn null_optional_fields_remain_unset() {
    let item=decode(&yaml::parse("kind: plan\nkey: free\nname: Free\nrank: 0\nfree: true\ndefault_free: null\npostpaid: null\nfeatures: null\n").unwrap()).unwrap();
    validate(&item).unwrap();
    assert_eq!(item.default_free, None);
    assert_eq!(item.postpaid, None);
    let g = grant(&yaml::parse("service: api\nkey: enabled\nenabled: true\nlimit: null").unwrap())
        .unwrap();
    validate_grants(&[g]).unwrap();
    let g = grant(
        &yaml::parse("service: api\nkey: enabled\nenabled: null\nconfiguration: null").unwrap(),
    )
    .unwrap();
    validate_grants(std::slice::from_ref(&g)).unwrap();
    assert!(normalize_grant(&g, 3, 1).is_err());
}
