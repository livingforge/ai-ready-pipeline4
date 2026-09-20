use arp4_cli::{data::*, excel};
use serde_json::json;

#[test]
fn strict_values_reject_duplicates_nonfinite_nonstring_keys_and_anchors() {
    for text in [
        "a: 1\na: 2",
        "1: value",
        "a: .inf",
        "a: &x 1\nb: *x",
        "a: &x 1",
    ] {
        assert!(parse(text, false).is_err(), "accepted {text}");
    }
    for text in [r#"{"a":1,"a":2}"#, r#"{"a":1e999}"#] {
        assert!(parse(text, true).is_err());
    }
}
#[test]
fn canonical_number_and_unicode_encoding() {
    let value = json!({"日本語":[1e-7,1e16,1.0,-0.0,true,null],"a":{}});
    assert_eq!(
        String::from_utf8(encoded(&value)).unwrap(),
        "{\n  \"a\": {},\n  \"日本語\": [\n    1e-07,\n    1e+16,\n    1.0,\n    -0.0,\n    true,\n    null\n  ]\n}\n"
    );
}
#[test]
fn portable_sheet_names_and_excel_bounds() {
    for (input, expected) in [
        ("CON", "%43ON.yml"),
        ("assets", "%61ssets.yml"),
        ("末尾. ", "末尾%2E%20.yml"),
        ("a%22b", "a%2522b.yml"),
    ] {
        assert_eq!(excel::filename(input), expected);
    }
    assert_eq!(excel::coordinate("XFD1048576").unwrap(), (16384, 1048576));
    for bad in [
        "XFE1",
        "A0",
        "A01",
        "A1048577",
        "a1",
        "A1x",
        "99999999999999999999999",
    ] {
        assert!(excel::coordinate(bad).is_err(), "{bad}");
    }
}
