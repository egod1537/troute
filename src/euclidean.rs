use crate::{api::LocationInput, matrix::TravelTimeMatrix, routing::TravelMode};

const EARTH_RADIUS_KILOMETERS: f64 = 6_371.0;

/// Builds a symmetric straight-line estimate in the minute unit expected by
/// the solver. Coordinates are projected onto a local tangent plane with the
/// equirectangular approximation and divided by a mode-specific nominal speed.
pub(crate) fn travel_time_rows(
    locations: &[LocationInput],
    travel_mode: TravelMode,
) -> Vec<Vec<u32>> {
    let speed = nominal_speed_kilometers_per_hour(travel_mode);
    locations
        .iter()
        .enumerate()
        .map(|(row, from)| {
            locations
                .iter()
                .enumerate()
                .map(|(column, to)| {
                    if row == column {
                        0
                    } else {
                        estimated_minutes(from, to, speed)
                    }
                })
                .collect()
        })
        .collect()
}

pub(crate) fn travel_time_matrix(
    locations: &[LocationInput],
    travel_mode: TravelMode,
) -> TravelTimeMatrix {
    TravelTimeMatrix::new(travel_time_rows(locations, travel_mode))
        .expect("validated locations produce a square matrix")
}

fn estimated_minutes(from: &LocationInput, to: &LocationInput, speed: f64) -> u32 {
    let from_latitude = from
        .latitude
        .expect("euclidean matrix locations have latitude")
        .to_radians();
    let to_latitude = to
        .latitude
        .expect("euclidean matrix locations have latitude")
        .to_radians();
    let latitude_delta = to_latitude - from_latitude;
    let longitude_delta = normalized_longitude_delta(
        from.longitude
            .expect("euclidean matrix locations have longitude"),
        to.longitude
            .expect("euclidean matrix locations have longitude"),
    )
    .to_radians();
    let x = longitude_delta * ((from_latitude + to_latitude) / 2.0).cos();
    let distance_kilometers =
        EARTH_RADIUS_KILOMETERS * (x.mul_add(x, latitude_delta * latitude_delta)).sqrt();
    let minutes = (distance_kilometers / speed * 60.0).ceil();

    if minutes <= 1.0 {
        1
    } else if minutes >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        minutes as u32
    }
}

fn normalized_longitude_delta(from: f64, to: f64) -> f64 {
    let delta = to - from;
    (delta + 180.0).rem_euclid(360.0) - 180.0
}

fn nominal_speed_kilometers_per_hour(mode: TravelMode) -> f64 {
    match mode {
        TravelMode::Walking => 4.8,
        TravelMode::Bicycling => 15.0,
        TravelMode::Transit => 30.0,
        TravelMode::Driving => 40.0,
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::domain::TimeOfDay;

    use super::*;

    fn location(id: &str, latitude: f64, longitude: f64) -> LocationInput {
        LocationInput {
            order_locked: false,
            id: id.to_owned(),
            name: None,
            place_id: String::new(),
            latitude: Some(latitude),
            longitude: Some(longitude),
            open_time: TimeOfDay::from_str("00:00").unwrap(),
            close_time: TimeOfDay::from_str("23:50").unwrap(),
            stay_minutes: 0,
            required: None,
            priority: None,
        }
    }

    #[test]
    fn builds_symmetric_minutes_from_straight_line_distance() {
        let locations = [location("A", 0.0, 0.0), location("B", 0.0, 0.01)];
        let matrix = travel_time_matrix(&locations, TravelMode::Walking);

        assert_eq!(matrix.travel_minutes(0, 0), Some(0));
        assert_eq!(matrix.travel_minutes(0, 1), Some(14));
        assert_eq!(matrix.travel_minutes(1, 0), Some(14));
    }

    #[test]
    fn travel_mode_changes_only_the_minute_scale() {
        let locations = [location("A", 37.5, 127.0), location("B", 37.5, 127.01)];
        let walking = travel_time_matrix(&locations, TravelMode::Walking);
        let driving = travel_time_matrix(&locations, TravelMode::Driving);

        assert!(walking.travel_minutes(0, 1) > driving.travel_minutes(0, 1));
    }

    #[test]
    fn crosses_the_antimeridian_by_the_short_path() {
        let locations = [location("A", 0.0, 179.9), location("B", 0.0, -179.9)];
        let matrix = travel_time_matrix(&locations, TravelMode::Driving);

        assert!(matrix.travel_minutes(0, 1).unwrap() < 60);
    }
}
