import type { RouteInput } from "./api";

let sampleSequence = 0;

export function createSample(): RouteInput {
  sampleSequence += 1;
  return {
    job_id: `route-testbed-${Date.now().toString(36)}-${sampleSequence.toString(36)}`,
    locations: [
      {
        id: "A",
        place_id: "place-a",
        open_time: "09:00",
        close_time: "18:00",
        stay_minutes: 60,
      },
      {
        id: "B",
        place_id: "place-b",
        open_time: "10:00",
        close_time: "20:00",
        stay_minutes: 90,
      },
      {
        id: "C",
        place_id: "place-c",
        open_time: "11:00",
        close_time: "19:00",
        stay_minutes: 45,
      },
    ],
    start_time: "09:00",
    debug: {
      min_job_duration_ms: 4_000,
    },
  };
}
