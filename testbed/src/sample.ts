import type { RouteInput } from "./api";

export const sample: RouteInput = {
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
  start_location_id: "A",
  start_time: "09:00",
};
