//! Behavior tests for derived validators, driven through the public
//! `derived_etag` / `matches_if_none_match` interface.

use pixtega::etag::{derived_etag, DERIVATION_GENERATION};
use pixtega::types::{ObjectIdentity, OutputFormat, Transform};

const TRANSFORM: Transform = Transform {
    width: 640,
    format: OutputFormat::Webp,
    quality: 75,
};

fn identity(validator: &str) -> ObjectIdentity {
    ObjectIdentity {
        validator: validator.to_string(),
        weak: false,
    }
}

#[test]
fn unreserved_validator_bytes_are_kept_and_all_others_percent_encoded() {
    let etag = derived_etag(&identity("aZ09-._~ /\"é"), &TRANSFORM);
    assert_eq!(
        etag.as_str(),
        format!("\"{DERIVATION_GENERATION}:640:webp:75:aZ09-._~%20%2F%22%C3%A9\"")
    );
}

#[test]
fn a_changed_upstream_validator_changes_the_derived_etag() {
    let before = derived_etag(&identity("v1"), &TRANSFORM);
    let after = derived_etag(&identity("v2"), &TRANSFORM);
    assert_ne!(before, after);
    assert!(!after.matches_if_none_match(before.as_str()));
}

#[test]
fn any_tag_in_an_if_none_match_list_matches() {
    let etag = derived_etag(&identity("v1"), &TRANSFORM);
    let tag = etag.as_str();
    assert!(etag.matches_if_none_match(&format!("\"other\", {tag}")));
    assert!(etag.matches_if_none_match(&format!("{tag}, \"other\"")));
    assert!(etag.matches_if_none_match(&format!("W/\"other\" , W/{tag}")));
    assert!(!etag.matches_if_none_match("\"other\", \"another\""));
}

#[test]
fn a_wildcard_or_empty_if_none_match_never_matches() {
    let etag = derived_etag(&identity("v1"), &TRANSFORM);
    assert!(!etag.matches_if_none_match("*"));
    assert!(!etag.matches_if_none_match("  "));
}
