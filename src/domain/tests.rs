use super::TimeOfDay;

#[test]
fn time_of_day_serializes_as_hhmm() {
    let time = TimeOfDay::from_minutes(13 * 60 + 45).unwrap();
    assert_eq!(serde_json::to_string(&time).unwrap(), r#""13:45""#);
}

#[test]
fn time_of_day_deserializes_valid_hhmm() {
    for (json, expected_minutes) in [
        (r#""00:00""#, 0),
        (r#""09:30""#, 9 * 60 + 30),
        (r#""23:59""#, 23 * 60 + 59),
    ] {
        let time: TimeOfDay = serde_json::from_str(json).unwrap();
        assert_eq!(time.minutes(), expected_minutes);
    }
}

#[test]
fn time_of_day_rejects_invalid_wire_values() {
    for json in [r#""9:30""#, r#""24:00""#, r#""09:60""#, "570"] {
        assert!(serde_json::from_str::<TimeOfDay>(json).is_err(), "{json}");
    }
}
