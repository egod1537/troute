use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use troute::{api::LocationInput, domain::TimeOfDay, DebugOptions, OptimizeRouteRequest};

pub const PLACE_FIXTURE_NAMES: [&str; 6] = [
    "tokyo_3_places",
    "tokyo_5_places",
    "tokyo_10_places",
    "seoul_3_places",
    "seoul_5_places",
    "seoul_10_places",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceFixture {
    pub name: String,
    pub description: Option<String>,
    pub region: String,
    pub timezone: String,
    pub default_start_time: Option<String>,
    pub verified_at: Option<String>,
    pub notes: Option<String>,
    pub locations: Vec<FixtureLocation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureLocation {
    pub id: String,
    pub name: String,
    pub place_id: String,
    pub address: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub verified_at: Option<String>,
}

#[derive(Debug, Error)]
pub enum FixtureError {
    #[error("cannot read place fixture {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON in place fixture {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid place fixture {fixture}: {message}")]
    Validation { fixture: String, message: String },
}

pub fn load_place_fixture(name: &str) -> Result<PlaceFixture, FixtureError> {
    let path = fixture_path(name);
    let contents = fs::read_to_string(&path).map_err(|source| FixtureError::Read {
        path: path.clone(),
        source,
    })?;
    let fixture: PlaceFixture =
        serde_json::from_str(&contents).map_err(|source| FixtureError::Json {
            path: path.clone(),
            source,
        })?;
    validate_place_fixture(&fixture)?;
    Ok(fixture)
}

pub fn build_optimize_request_from_fixture(
    name: &str,
    job_id: impl Into<String>,
) -> Result<OptimizeRouteRequest, FixtureError> {
    let fixture = load_place_fixture(name)?;
    let start_time = fixture.default_start_time.as_deref().unwrap_or("09:00");
    let start_time = TimeOfDay::from_str(start_time).map_err(|error| FixtureError::Validation {
        fixture: fixture.name.clone(),
        message: format!("invalid default_start_time: {error}"),
    })?;
    let open_time = TimeOfDay::from_minutes(0).expect("midnight is a valid time");
    let close_time = TimeOfDay::from_minutes(23 * 60 + 50).expect("23:50 is a valid time");

    Ok(OptimizeRouteRequest {
        job_id: job_id.into(),
        locations: fixture
            .locations
            .into_iter()
            .map(|location| LocationInput {
                id: location.id,
                name: Some(location.name),
                place_id: location.place_id,
                open_time,
                close_time,
                stay_minutes: 0,
            })
            .collect(),
        start_time,
        travel_mode: None,
        travel_time_matrix: None,
        debug: None::<DebugOptions>,
    })
}

pub fn validate_place_fixture(fixture: &PlaceFixture) -> Result<(), FixtureError> {
    let invalid = |message: String| FixtureError::Validation {
        fixture: fixture.name.clone(),
        message,
    };

    if fixture.name.trim().is_empty() {
        return Err(invalid("name must not be empty".to_owned()));
    }
    if fixture.region.trim().is_empty() {
        return Err(invalid("region must not be empty".to_owned()));
    }
    if fixture.timezone.trim().is_empty() {
        return Err(invalid("timezone must not be empty".to_owned()));
    }
    match (fixture.region.as_str(), fixture.timezone.as_str()) {
        ("JP", "Asia/Tokyo") | ("KR", "Asia/Seoul") => {}
        _ => {
            return Err(invalid(format!(
                "unsupported region/timezone pair: {}/{}",
                fixture.region, fixture.timezone
            )))
        }
    }
    if fixture.locations.len() < 2 {
        return Err(invalid(
            "locations must contain at least two places".to_owned(),
        ));
    }
    if let Some(value) = fixture.default_start_time.as_deref() {
        TimeOfDay::from_str(value)
            .map_err(|error| invalid(format!("invalid default_start_time: {error}")))?;
    }

    let mut ids = HashSet::with_capacity(fixture.locations.len());
    let mut place_ids = HashSet::with_capacity(fixture.locations.len());
    for (index, location) in fixture.locations.iter().enumerate() {
        if location.id.trim().is_empty() {
            return Err(invalid(format!("locations[{index}].id must not be empty")));
        }
        if location.name.trim().is_empty() {
            return Err(invalid(format!(
                "locations[{index}].name must not be empty"
            )));
        }
        if location.place_id.trim().is_empty() {
            return Err(invalid(format!(
                "locations[{index}].place_id must not be empty"
            )));
        }
        if !ids.insert(location.id.as_str()) {
            return Err(invalid(format!("duplicate location id: {}", location.id)));
        }
        if !place_ids.insert(location.place_id.as_str()) {
            return Err(invalid(format!(
                "duplicate place_id: {}",
                location.place_id
            )));
        }
        if location.latitude.is_some() != location.longitude.is_some() {
            return Err(invalid(format!(
                "location {} must provide both latitude and longitude",
                location.id
            )));
        }
    }
    Ok(())
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/places")
        .join(format!("{name}.json"))
}
