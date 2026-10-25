// SPDX-License-Identifier: FSL-1.1-ALv2
mod definition;
mod state;
pub use definition::*;
pub use state::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeteringError {
    Definition(String),
    Event(String),
    NoMatch,
    Revision(String),
    Limit(String),
}
impl std::fmt::Display for MeteringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Definition(s) => write!(f, "invalid meter definition: {s}"),
            Self::Event(s) => write!(f, "invalid meter event: {s}"),
            Self::NoMatch => write!(f, "no meter matches event"),
            Self::Revision(s) => write!(f, "invalid meter revision: {s}"),
            Self::Limit(s) => write!(f, "meter cardinality limit exceeded: {s}"),
        }
    }
}
impl std::error::Error for MeteringError {}
pub type Result<T> = std::result::Result<T, MeteringError>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::Decimal;
    use std::collections::BTreeMap;
    fn number(s: &str) -> Value {
        Value::number(s).unwrap()
    }
    fn definition(key: &str, kind: Aggregation) -> Definition {
        Definition {
            key: key.into(),
            event_type: "use".into(),
            aggregation: kind,
            value_attribute: if kind == Aggregation::Count { "" } else { "n" }.into(),
            group_by: vec![],
            filters: vec![],
        }
    }
    fn event(n: &str) -> Event {
        Event {
            event_type: "use".into(),
            attributes: BTreeMap::from([("n".into(), number(n))]),
        }
    }
    #[test]
    fn canonical_groups_preserve_names_and_scalar_types() {
        let values = BTreeMap::from([
            ("z".into(), number("1.00")),
            ("a".into(), Value::string("<&>\u{2028}\u{2029}")),
        ]);
        assert_eq!(
            group_key(&values).unwrap(),
            format!(
                r#"[{{"name":"a","kind":"string","value":"<&>{}{}"}},{{"name":"z","kind":"number","value":"1"}}]"#,
                '\u{2028}', '\u{2029}'
            )
        );
        let mut changed = values.clone();
        changed.insert("z".into(), Value::string("1"));
        assert_ne!(group_key(&values), group_key(&changed));
        assert_eq!(group_key(&BTreeMap::new()).unwrap(), "[]");
        assert!(group_key(&BTreeMap::from([("".into(), number("1"))])).is_err());
        for raw in ["null", "[]", "{}"] {
            assert!(parse_value(&serde_json::from_str(raw).unwrap()).is_err());
        }
    }
    #[test]
    fn typed_filters_and_numeric_boundaries() {
        use FilterOperator::*;
        for (operator, wanted, actual, expected) in [
            (Equal, number("5.00"), number("5"), true),
            (Equal, Value::string("5"), number("5"), false),
            (NotEqual, number("5"), Value::string("5"), true),
            (Equal, Value::boolean(true), Value::string("true"), false),
            (GreaterThan, number("1"), number("1"), false),
            (GreaterThanOrEqual, number("1"), number("1"), true),
            (LessThan, number("1"), number("1"), false),
            (LessThanOrEqual, number("1"), number("1"), true),
            (
                GreaterThan,
                number("0.999999999999999999999999"),
                number("1"),
                true,
            ),
        ] {
            let mut d = definition("m", Aggregation::Count);
            d.filters.push(Filter {
                attribute: "n".into(),
                operator,
                value: Some(wanted),
                values: vec![],
            });
            let e = Event {
                event_type: "use".into(),
                attributes: BTreeMap::from([("n".into(), actual)]),
            };
            assert_eq!(match_event(&[d.clone()], &e).is_ok(), expected);
            assert_eq!(
                match_event(
                    &[d],
                    &Event {
                        event_type: "use".into(),
                        ..Default::default()
                    }
                )
                .unwrap_err(),
                MeteringError::NoMatch
            );
        }
        let mut d = definition("m", Aggregation::Count);
        d.filters = vec![
            Filter {
                attribute: "n".into(),
                operator: In,
                value: None,
                values: vec![Value::string("x"), number("5.00"), Value::boolean(true)],
            },
            Filter {
                attribute: "missing".into(),
                operator: Exists,
                value: Some(Value::boolean(false)),
                values: vec![],
            },
        ];
        assert!(match_event(&[d.clone()], &event("5")).is_ok());
        d.filters[0].values = vec![Value::string("5")];
        assert_eq!(
            match_event(&[d], &event("5")).unwrap_err(),
            MeteringError::NoMatch
        );
    }
    #[test]
    fn definitions_reject_invalid_shapes_and_duplicates() {
        let mut d = definition("m", Aggregation::Sum);
        d.value_attribute.clear();
        assert!(d.validate().is_err());
        let mut d = definition("m", Aggregation::Count);
        d.value_attribute = "n".into();
        assert!(d.validate().is_err());
        let mut d = definition("m", Aggregation::Count);
        d.group_by = vec!["a".into(), "a".into()];
        assert!(d.validate().is_err());
        let d = definition("m", Aggregation::Count);
        assert!(match_event(&[d.clone(), d], &event("1")).is_err());
        for (operator, value, values) in [
            (FilterOperator::In, None, vec![]),
            (FilterOperator::Exists, Some(Value::string("true")), vec![]),
            (
                FilterOperator::GreaterThan,
                Some(Value::string("1")),
                vec![],
            ),
            (FilterOperator::Equal, Some(number("1")), vec![number("1")]),
        ] {
            let mut d = definition("m", Aggregation::Count);
            d.filters.push(Filter {
                attribute: "n".into(),
                operator,
                value,
                values,
            });
            assert!(d.validate().is_err());
        }
    }
    #[test]
    fn six_aggregations_and_rounding() {
        let defs: Vec<_> = [
            ("count", Aggregation::Count),
            ("sum", Aggregation::Sum),
            ("min", Aggregation::Min),
            ("max", Aggregation::Max),
            ("avg", Aggregation::Avg),
            ("unique", Aggregation::UniqueCount),
        ]
        .into_iter()
        .map(|(k, a)| definition(k, a))
        .collect();
        let active = BTreeMap::from([
            ("a".into(), event("-10.1")),
            ("b".into(), event("0.2")),
            ("c".into(), event("0.3")),
        ]);
        let state = State::build(&defs, &active).unwrap();
        for (k, want) in [
            ("count", "3"),
            ("sum", "-9.6"),
            ("min", "-10.1"),
            ("max", "0.3"),
            ("avg", "-3.2"),
            ("unique", "3"),
        ] {
            assert_eq!(state.aggregates()[k]["[]"].quantity().to_string(), want);
        }
        assert_eq!(state.aggregates()["avg"]["[]"].sum.to_string(), "-9.6");
        for (n, want) in [
            ("0.0000005", "0.000001"),
            ("-0.0000005", "-0.000001"),
            ("0.00000049", "0"),
            ("-0.00000049", "0"),
            ("1.2345675", "1.234568"),
            ("-1.2345675", "-1.234568"),
        ] {
            assert_eq!(
                rounded_average(&n.parse::<Decimal>().unwrap(), 1).to_string(),
                want
            );
        }
        assert_eq!(
            rounded_average(&"0.000001".parse().unwrap(), 2).to_string(),
            "0.000001"
        );
        assert_eq!(
            rounded_average(&"1".parse().unwrap(), 3).to_string(),
            "0.333333"
        );
        assert_eq!(state.aggregates()["sum"]["[]"].distinct_count, 0);
        assert_eq!(state.aggregates()["avg"]["[]"].distinct_count, 0);
    }
    #[test]
    fn revisions_recompute_extrema_and_reject_whole_invalid_event() {
        let defs: Vec<_> = [
            ("min", Aggregation::Min),
            ("max", Aggregation::Max),
            ("sum", Aggregation::Sum),
            ("unique", Aggregation::UniqueCount),
        ]
        .into_iter()
        .map(|(k, a)| definition(k, a))
        .collect();
        let state = State::build(
            &defs,
            &BTreeMap::from([
                ("a".into(), event("-3")),
                ("b".into(), event("-3.0")),
                ("c".into(), event("5")),
            ]),
        )
        .unwrap();
        let (next, _) = state
            .apply(Revision {
                kind: RevisionKind::Replace,
                event_id: "a".into(),
                event: Some(event("8")),
            })
            .unwrap();
        assert_eq!(next.aggregates()["min"]["[]"].value.to_string(), "-3");
        assert_eq!(next.aggregates()["max"]["[]"].value.to_string(), "8");
        let (next, _) = next
            .apply(Revision {
                kind: RevisionKind::Cancel,
                event_id: "b".into(),
                event: None,
            })
            .unwrap();
        assert_eq!(next.aggregates()["min"]["[]"].value.to_string(), "5");
        assert_eq!(next.aggregates()["sum"]["[]"].sum.to_string(), "13");
        assert_eq!(state.aggregates()["sum"]["[]"].sum.to_string(), "-1");
        assert!(
            next.apply(Revision {
                kind: RevisionKind::Insert,
                event_id: "bad".into(),
                event: Some(Event {
                    event_type: "use".into(),
                    ..Default::default()
                })
            })
            .is_err()
        );
        for (kind, id) in [
            (RevisionKind::Insert, "a"),
            (RevisionKind::Replace, "missing"),
            (RevisionKind::Cancel, ""),
        ] {
            assert!(
                next.apply(Revision {
                    kind,
                    event_id: id.into(),
                    event: Some(event("1"))
                })
                .is_err()
            );
        }
    }
    #[test]
    fn cardinality_and_empty_group_removal() {
        let mut d = definition("unique", Aggregation::UniqueCount);
        d.group_by = vec!["region".into()];
        let grouped = |region: &str, n: &str| {
            let mut e = event(n);
            e.attributes.insert("region".into(), Value::string(region));
            e
        };
        let s = State::build(&[d], &BTreeMap::new()).unwrap();
        assert!(s.cardinality()["unique"].groups.is_empty());
        let (s, changes) = s
            .apply(Revision {
                kind: RevisionKind::Insert,
                event_id: "a".into(),
                event: Some(grouped("eu", "1")),
            })
            .unwrap();
        assert!(changes[0].before.is_none());
        let (s, _) = s
            .apply(Revision {
                kind: RevisionKind::Insert,
                event_id: "b".into(),
                event: Some(grouped("eu", "1.0")),
            })
            .unwrap();
        s.check_limits(Limits {
            max_groups_per_meter: 1,
            max_distinct_values_per_group: 1,
        })
        .unwrap();
        let (s, changes) = s
            .apply(Revision {
                kind: RevisionKind::Replace,
                event_id: "a".into(),
                event: Some(grouped("us", "2")),
            })
            .unwrap();
        assert_eq!(changes.len(), 2);
        assert!(
            s.check_limits(Limits {
                max_groups_per_meter: 1,
                ..Default::default()
            })
            .is_err()
        );
        let (s, changes) = s
            .apply(Revision {
                kind: RevisionKind::Cancel,
                event_id: "b".into(),
                event: None,
            })
            .unwrap();
        assert!(changes[0].after.is_none());
        s.check_limits(Limits {
            max_groups_per_meter: 1,
            max_distinct_values_per_group: 1,
        })
        .unwrap();
        let (s, _) = s
            .apply(Revision {
                kind: RevisionKind::Insert,
                event_id: "c".into(),
                event: Some(grouped("us", "3")),
            })
            .unwrap();
        assert!(
            s.check_limits(Limits {
                max_distinct_values_per_group: 1,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            s.check_limits(Limits {
                max_groups_per_meter: -1,
                ..Default::default()
            })
            .is_err()
        );
    }
}
