import type { RouteInput } from "./api";

let sampleSequence = 0;

export function createSample(): RouteInput {
  sampleSequence += 1;
  return {
    job_id: `route-testbed-${Date.now().toString(36)}-${sampleSequence.toString(36)}`,
    locations: [
      {
        id: "A",
        name: "Start",
        place_id: "place-a",
        open_time: "09:00",
        close_time: "18:00",
        stay_minutes: 60,
      },
      {
        id: "B",
        name: "Waypoint",
        place_id: "place-b",
        open_time: "10:00",
        close_time: "20:00",
        stay_minutes: 90,
      },
      {
        id: "C",
        name: "Destination",
        place_id: "place-c",
        open_time: "11:00",
        close_time: "19:00",
        stay_minutes: 40,
      },
    ],
    start_time: "00:00",
    travel_time_matrix: [
      [0, 30, 45],
      [28, 0, 15],
      [40, 18, 0],
    ],
    debug: {
      min_job_duration_ms: 4_000,
    },
  };
}
