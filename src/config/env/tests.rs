use std::collections::BTreeMap;

use super::EnvReader;

fn source(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn bounded_size_handles_missing_valid_invalid_and_out_of_range_values() {
    let empty = source(&[]);
    assert_eq!(
        EnvReader::new(&empty).read_bounded_size("LIMIT", 7, 10),
        Ok(7)
    );
    let valid = source(&[("LIMIT", "9")]);
    assert_eq!(
        EnvReader::new(&valid).read_bounded_size("LIMIT", 7, 10),
        Ok(9)
    );
    for value in ["invalid", " 9 ", "11", "184467440737095516160"] {
        let invalid = source(&[("LIMIT", value)]);
        assert_eq!(
            EnvReader::new(&invalid).read_bounded_size("LIMIT", 7, 10),
            Err("LIMIT must be an integer between 1 and 10".to_owned())
        );
    }
}

#[test]
fn positive_u64_preserves_defaults_and_validation() {
    let empty = source(&[]);
    assert_eq!(
        EnvReader::new(&empty).read_positive_u64("TIMEOUT", 5),
        Ok(5)
    );
    let valid = source(&[("TIMEOUT", "42")]);
    assert_eq!(
        EnvReader::new(&valid).read_positive_u64("TIMEOUT", 5),
        Ok(42)
    );
    for value in ["invalid", " 42 ", "0"] {
        let invalid = source(&[("TIMEOUT", value)]);
        assert_eq!(
            EnvReader::new(&invalid).read_positive_u64("TIMEOUT", 5),
            Err("TIMEOUT must be a positive integer".to_owned())
        );
    }
}

#[test]
fn list_bool_and_float_parsers_are_typed_and_validated() {
    let values = source(&[("SEEDS", "3, 5,8"), ("ENABLED", "yes"), ("RATIO", "0.75")]);
    let reader = EnvReader::new(&values);
    assert_eq!(reader.read_u64_list("SEEDS", &[1, 2]), Ok(vec![3, 5, 8]));
    assert_eq!(reader.read_bool("ENABLED", false), Ok(true));
    assert_eq!(reader.read_f64("RATIO", 0.5, 0.0, 1.0), Ok(0.75));

    let whitespace = source(&[("ENABLED", " TrUe "), ("RATIO", " 0.75 ")]);
    let reader = EnvReader::new(&whitespace);
    assert_eq!(reader.read_bool("ENABLED", false), Ok(true));
    assert!(reader.read_f64("RATIO", 0.5, 0.0, 1.0).is_err());

    let invalid = source(&[("SEEDS", "3,bad"), ("ENABLED", "sometimes"), ("RATIO", "2")]);
    let reader = EnvReader::new(&invalid);
    assert_eq!(
        reader.read_u64_list("SEEDS", &[1, 2]),
        Err("SEEDS must be a comma-separated list of u64 seeds".to_owned())
    );
    let empty_item = source(&[("SEEDS", "3,,4")]);
    assert_eq!(
        EnvReader::new(&empty_item).read_u64_list("SEEDS", &[1]),
        Err("SEEDS must be a comma-separated list of u64 seeds".to_owned())
    );
    assert!(reader.read_bool("ENABLED", false).is_err());
    assert_eq!(
        reader.read_f64("RATIO", 0.5, 0.0, 1.0),
        Err("RATIO must be finite and in (0, 1]".to_owned())
    );

    for (value, expected) in [
        ("true", true),
        ("TRUE", true),
        ("1", true),
        ("on", true),
        ("false", false),
        ("FALSE", false),
        ("0", false),
        ("off", false),
    ] {
        let value = source(&[("ENABLED", value)]);
        assert_eq!(
            EnvReader::new(&value).read_bool("ENABLED", !expected),
            Ok(expected)
        );
    }

    for special in ["NaN", "inf", "-inf"] {
        let value = source(&[("RATIO", special)]);
        assert!(EnvReader::new(&value)
            .read_f64("RATIO", 0.5, 0.0, 1.0)
            .is_err());
    }
}
