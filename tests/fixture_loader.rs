mod support;

use std::collections::HashSet;

use support::place_fixtures::{
    build_optimize_request_from_fixture, load_place_fixture, validate_place_fixture,
    PLACE_FIXTURE_NAMES,
};
use troute::domain::{OptimizationProblem, RoutingReference};

#[test]
fn all_place_fixtures_parse_and_have_the_declared_size() {
    for name in PLACE_FIXTURE_NAMES {
        let fixture = load_place_fixture(name).unwrap_or_else(|error| panic!("{name}: {error}"));
        let expected_count: usize = name
            .split('_')
            .nth(1)
            .expect("fixture name contains its size")
            .parse()
            .expect("fixture size is numeric");

        assert_eq!(fixture.locations.len(), expected_count, "{name}");
    }
}

#[test]
fn smaller_city_fixtures_are_ordered_prefixes_of_the_ten_place_fixture() {
    for city in ["tokyo", "seoul"] {
        let complete = load_place_fixture(&format!("{city}_10_places")).unwrap();
        for size in [3, 5] {
            let fixture = load_place_fixture(&format!("{city}_{size}_places")).unwrap();
            assert_eq!(
                fixture.locations,
                complete.locations[..size],
                "{city} {size}"
            );
        }
    }
}

#[test]
fn canonical_city_sets_do_not_reuse_ids_or_place_ids() {
    let fixtures = [
        load_place_fixture("tokyo_10_places").unwrap(),
        load_place_fixture("seoul_10_places").unwrap(),
    ];
    let mut ids = HashSet::new();
    let mut place_ids = HashSet::new();

    for fixture in fixtures {
        for location in fixture.locations {
            assert!(
                ids.insert(location.id.clone()),
                "duplicate id: {}",
                location.id
            );
            assert!(
                place_ids.insert(location.place_id.clone()),
                "duplicate place_id: {}",
                location.place_id
            );
        }
    }
}

#[test]
fn fixture_helper_builds_a_valid_optimization_problem_without_network_access() {
    let fixture = load_place_fixture("tokyo_3_places").unwrap();
    let request = build_optimize_request_from_fixture("tokyo_3_places", "fixture-test").unwrap();
    let problem = OptimizationProblem::try_from(request).unwrap();

    assert_eq!(problem.locations().len(), fixture.locations.len());
    assert_eq!(problem.start_time().to_string(), "09:00");
    for (actual, expected) in problem.locations().iter().zip(fixture.locations) {
        assert_eq!(actual.id(), expected.id);
        assert_eq!(
            actual.routing_reference(),
            &RoutingReference::GooglePlaceId(expected.place_id)
        );
    }
}

#[test]
fn fixture_validation_rejects_duplicate_ids_and_place_ids() {
    let fixture = load_place_fixture("tokyo_3_places").unwrap();

    let mut duplicate_id = fixture.clone();
    duplicate_id.locations[1].id = duplicate_id.locations[0].id.clone();
    assert!(validate_place_fixture(&duplicate_id)
        .unwrap_err()
        .to_string()
        .contains("duplicate location id"));

    let mut duplicate_place_id = fixture;
    duplicate_place_id.locations[1].place_id = duplicate_place_id.locations[0].place_id.clone();
    assert!(validate_place_fixture(&duplicate_place_id)
        .unwrap_err()
        .to_string()
        .contains("duplicate place_id"));
}

#[test]
fn fixture_validation_rejects_missing_place_id() {
    let mut fixture = load_place_fixture("seoul_3_places").unwrap();
    fixture.locations[0].place_id.clear();

    assert!(validate_place_fixture(&fixture)
        .unwrap_err()
        .to_string()
        .contains("place_id must not be empty"));
}
