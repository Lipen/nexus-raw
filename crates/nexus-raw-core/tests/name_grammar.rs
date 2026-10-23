//! The shared name grammar fixture: the single table both the Rust core and the
//! Python reference client execute, so a grammar change cannot drift one client
//! away from the other.

use nexus_raw_core::{ArtifactName, Error};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/name-grammar.json"))
        .expect("the name fixture parses as JSON")
}

#[test]
fn every_fixture_name_matches_the_grammar() {
    let fixture = fixture();
    let valid = fixture["names"]["valid"]
        .as_array()
        .expect("names.valid is an array");
    for name in valid {
        let name = name.as_str().expect("a fixture name is a string");
        ArtifactName::parse(name).unwrap_or_else(|e| panic!("{name:?} must be valid: {e}"));
    }
    let invalid = fixture["names"]["invalid"]
        .as_array()
        .expect("names.invalid is an array");
    for item in invalid {
        let name = item["name"].as_str().expect("a fixture name is a string");
        let err = ArtifactName::parse(name)
            .expect_err("an invalid fixture name must be refused by the grammar");
        assert!(
            matches!(err, Error::UnsafeName { .. }),
            "{name:?} must fail the grammar, got {err:?}"
        );
    }
}
