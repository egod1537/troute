import seoul5 from "../../tests/fixtures/places/seoul_5_places.json";
import tokyo10 from "../../tests/fixtures/places/tokyo_10_places.json";
import tokyo3 from "../../tests/fixtures/places/tokyo_3_places.json";
import tokyo5 from "../../tests/fixtures/places/tokyo_5_places.json";

export interface JobPresetLocation {
  id: string;
  name: string;
  placeId: string;
  openTime: string;
  closeTime: string;
  stayMinutes: number;
}

export interface JobPreset {
  key: "tokyo-3" | "tokyo-5" | "tokyo-10" | "seoul-5" | "synthetic-8";
  name: string;
  kind: "places" | "synthetic";
  locations: JobPresetLocation[];
  travelTimeMatrix?: number[][];
}

interface PlaceFixture {
  name: string;
  locations: { id: string; name: string; place_id: string }[];
}

function placePreset(
  key: JobPreset["key"],
  fixture: PlaceFixture,
): JobPreset {
  return {
    key,
    name: fixture.name.replace(" Places", ""),
    kind: "places",
    locations: fixture.locations.map(({ id, name, place_id }, index) => ({
      id,
      name,
      placeId: place_id,
      openTime: "09:00",
      closeTime: "18:00",
      stayMinutes:
        index === 0 || index === fixture.locations.length - 1 ? 0 : 60,
    })),
  };
}

const SYNTHETIC_8: JobPreset = {
  key: "synthetic-8",
  name: "Synthetic 8",
  kind: "synthetic",
  locations: "ABCDEFGH".split("").map((id, index) => ({
    id,
    name:
      index === 0
        ? "Synthetic Start"
        : index === 7
          ? "Synthetic Destination"
          : `Synthetic Stop ${id}`,
    placeId: "",
    openTime: index === 0 ? "08:00" : "09:00",
    closeTime: index === 7 ? "22:00" : "20:00",
    stayMinutes: index === 0 || index === 7 ? 0 : 30,
  })),
  // Deliberately asymmetric: every directed pair is independently defined.
  travelTimeMatrix: [
    [0, 12, 25, 18, 34, 21, 29, 40],
    [17, 0, 14, 27, 19, 31, 22, 35],
    [23, 16, 0, 11, 26, 20, 33, 28],
    [15, 24, 13, 0, 12, 29, 18, 30],
    [32, 17, 28, 14, 0, 15, 25, 21],
    [20, 30, 19, 26, 16, 0, 10, 24],
    [27, 21, 35, 17, 23, 12, 0, 13],
    [38, 33, 26, 31, 20, 22, 16, 0],
  ],
};

export const JOB_PRESETS: JobPreset[] = [
  placePreset("tokyo-3", tokyo3),
  placePreset("tokyo-5", tokyo5),
  placePreset("tokyo-10", tokyo10),
  placePreset("seoul-5", seoul5),
  SYNTHETIC_8,
];
