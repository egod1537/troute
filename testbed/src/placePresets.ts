import type { StartPolicy, TravelMode, VisitPolicy } from "./api";
import seoul10 from "../../tests/fixtures/places/seoul_10_places.json";
import seoul3 from "../../tests/fixtures/places/seoul_3_places.json";
import seoul5 from "../../tests/fixtures/places/seoul_5_places.json";

export interface JobPresetLocation {
  id: string;
  name: string;
  placeId: string;
  latitude?: number;
  longitude?: number;
  openTime: string;
  closeTime: string;
  stayMinutes: number;
  required?: boolean;
  priority?: number;
}

export interface JobPreset {
  key:
    | "seoul-3-quick"
    | "seoul-5-core"
    | "seoul-5-windows"
    | "seoul-5-partial"
    | "seoul-10-day"
    | "synthetic-8";
  name: string;
  description: string;
  kind: "places" | "synthetic";
  countryCode?: string;
  travelMode?: TravelMode;
  startPolicy?: StartPolicy;
  startTime?: string;
  visitPolicy?: VisitPolicy;
  locations: JobPresetLocation[];
  travelTimeMatrix?: number[][];
}

interface PlaceFixtureLocation {
  id: string;
  name: string;
  place_id: string;
  latitude: number;
  longitude: number;
}

interface PlaceFixture {
  locations: PlaceFixtureLocation[];
}

export interface SeoulLocationCoordinate {
  name: string;
  latitude: number;
  longitude: number;
}

const SEOUL_LOCATION_NAMES: Record<string, string> = {
  "seoul-station": "서울역",
  "gangnam-station": "강남역",
  "hongik-university-station": "홍대입구역",
  "gyeongbokgung-palace": "경복궁",
  "myeongdong-station": "명동역",
  "dongdaemun-design-plaza": "동대문디자인플라자",
  "jamsil-station": "잠실역",
  "yeouido-station": "여의도역",
  "itaewon-station": "이태원역",
  "starfield-coex-mall": "스타필드 코엑스몰",
};

export const SEOUL_LOCATION_COORDINATES: Record<string, SeoulLocationCoordinate> =
  Object.fromEntries(
    seoul10.locations.map((location) => [
      location.id,
      {
        name: SEOUL_LOCATION_NAMES[location.id] ?? location.name,
        latitude: location.latitude,
        longitude: location.longitude,
      },
    ]),
  );

const EARTH_RADIUS_KILOMETERS = 6_371;

function distanceKilometers(
  from: PlaceFixtureLocation,
  to: PlaceFixtureLocation,
): number {
  const radians = (degrees: number) => degrees * Math.PI / 180;
  const latitudeDelta = radians(to.latitude - from.latitude);
  const longitudeDelta = radians(to.longitude - from.longitude);
  const fromLatitude = radians(from.latitude);
  const toLatitude = radians(to.latitude);
  const haversine =
    Math.sin(latitudeDelta / 2) ** 2 +
    Math.cos(fromLatitude) * Math.cos(toLatitude) *
      Math.sin(longitudeDelta / 2) ** 2;
  return 2 * EARTH_RADIUS_KILOMETERS * Math.asin(Math.sqrt(haversine));
}

function estimatedTravelMinutes(
  from: PlaceFixtureLocation,
  to: PlaceFixtureLocation,
  mode: TravelMode,
): number {
  const distance = distanceKilometers(from, to);
  if (mode === "WALKING") {
    return Math.max(1, Math.ceil(distance * 1.25 / 4.8 * 60));
  }
  if (mode === "BICYCLING") {
    return Math.max(1, Math.ceil(distance * 1.2 / 14 * 60));
  }
  if (mode === "DRIVING") {
    return Math.max(1, Math.ceil(distance * 1.35 / 28 * 60 + 3));
  }
  // Offline demo estimate: access/transfer time plus an urban transit average.
  return Math.max(1, Math.ceil(distance * 1.35 / 24 * 60 + 8));
}

function estimatedTravelMatrix(
  locations: PlaceFixtureLocation[],
  mode: TravelMode,
): number[][] {
  return locations.map((from, row) =>
    locations.map((to, column) =>
      row === column ? 0 : estimatedTravelMinutes(from, to, mode),
    ),
  );
}

interface PlacePresetOptions {
  key: JobPreset["key"];
  name: string;
  description: string;
  fixture: PlaceFixture;
  locationOrder?: string[];
  travelMode?: TravelMode;
  startPolicy?: StartPolicy;
  startTime?: string;
  visitPolicy?: VisitPolicy;
  schedule?: (
    location: PlaceFixtureLocation,
    index: number,
    count: number,
  ) => Partial<
    Pick<
      JobPresetLocation,
      "openTime" | "closeTime" | "stayMinutes" | "required" | "priority"
    >
  >;
}

function placePreset(options: PlacePresetOptions): JobPreset {
  const fixtureById = new Map(
    options.fixture.locations.map((location) => [location.id, location]),
  );
  const locations = options.locationOrder
    ? options.locationOrder.map((id) => {
        const location = fixtureById.get(id);
        if (!location) throw new Error(`Unknown Seoul fixture location: ${id}`);
        return location;
      })
    : options.fixture.locations;
  const travelMode = options.travelMode ?? "TRANSIT";

  return {
    key: options.key,
    name: options.name,
    description: options.description,
    kind: "places",
    countryCode: "KR",
    travelMode,
    startPolicy: options.startPolicy ?? "EARLIEST",
    startTime: options.startTime ?? "09:00",
    visitPolicy: options.visitPolicy,
    travelTimeMatrix: estimatedTravelMatrix(locations, travelMode),
    locations: locations.map((location, index) => ({
      id: location.id,
      name: SEOUL_LOCATION_NAMES[location.id] ?? location.name,
      placeId: location.place_id,
      latitude: location.latitude,
      longitude: location.longitude,
      openTime: "00:00",
      closeTime: "23:50",
      stayMinutes: index === 0 || index === locations.length - 1 ? 0 : 30,
      ...options.schedule?.(location, index, locations.length),
    })),
  };
}

const SEOUL_3_QUICK = placePreset({
  key: "seoul-3-quick",
  name: "서울 빠른 확인 · 3",
  description: "서울역·강남·홍대 기본 동작",
  fixture: seoul3,
});

const SEOUL_5_CORE = placePreset({
  key: "seoul-5-core",
  name: "서울 핵심 명소 · 5",
  description: "도심 주요 지점 경로 최적화",
  fixture: seoul5,
  schedule: (_location, index, count) => ({
    stayMinutes: index === 0 || index === count - 1 ? 0 : 50,
  }),
});

const SEOUL_5_WINDOWS = placePreset({
  key: "seoul-5-windows",
  name: "서울 시간 제약 · 5",
  description: "운영시간·대기·체류 계산",
  fixture: seoul5,
  locationOrder: [
    "seoul-station",
    "gyeongbokgung-palace",
    "myeongdong-station",
    "hongik-university-station",
    "gangnam-station",
  ],
  schedule: (location, index, count) => {
    if (index === 0 || index === count - 1) return { stayMinutes: 0 };
    if (location.id === "gyeongbokgung-palace") {
      return { openTime: "09:00", closeTime: "17:00", stayMinutes: 90 };
    }
    if (location.id === "myeongdong-station") {
      return { openTime: "10:00", closeTime: "21:00", stayMinutes: 60 };
    }
    return { openTime: "11:00", closeTime: "22:00", stayMinutes: 80 };
  },
});

const SEOUL_5_PARTIAL = placePreset({
  key: "seoul-5-partial",
  name: "서울 전체 방문 불가 · 5",
  description: "좁은 시간창에서 일부 장소 생략 검증",
  fixture: seoul5,
  visitPolicy: "BEST_EFFORT_ON_FAILURE",
  startPolicy: "FIXED",
  startTime: "09:00",
  locationOrder: [
    "seoul-station",
    "gyeongbokgung-palace",
    "myeongdong-station",
    "hongik-university-station",
    "gangnam-station",
  ],
  schedule: (_location, index, count) => {
    const endpoint = index === 0 || index === count - 1;
    if (endpoint) {
      return {
        openTime: "09:00",
        closeTime: "23:50",
        stayMinutes: 0,
        required: true,
        priority: 100,
      };
    }
    return {
      openTime: "09:00",
      closeTime: "10:30",
      stayMinutes: 60,
      required: false,
      priority: 100 - index * 10,
    };
  },
});

const SEOUL_10_DAY = placePreset({
  key: "seoul-10-day",
  name: "서울 종일 투어 · 10",
  description: "10개 지점 성능·순서 비교",
  fixture: seoul10,
  startTime: "08:30",
  schedule: (location, index, count) => {
    if (index === 0) {
      return { openTime: "08:30", closeTime: "23:50", stayMinutes: 0 };
    }
    if (index === count - 1) {
      return { openTime: "09:00", closeTime: "22:00", stayMinutes: 0 };
    }
    if (location.id === "gyeongbokgung-palace") {
      return { openTime: "09:00", closeTime: "17:00", stayMinutes: 60 };
    }
    if (location.id === "dongdaemun-design-plaza") {
      return { openTime: "10:00", closeTime: "20:00", stayMinutes: 50 };
    }
    return { openTime: "09:00", closeTime: "22:00", stayMinutes: 40 };
  },
});

const SYNTHETIC_8: JobPreset = {
  key: "synthetic-8",
  name: "Synthetic 8",
  description: "비대칭 행렬 알고리즘 스트레스 테스트",
  kind: "synthetic",
  startPolicy: "EARLIEST",
  startTime: "08:00",
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
  SEOUL_3_QUICK,
  SEOUL_5_CORE,
  SEOUL_5_WINDOWS,
  SEOUL_5_PARTIAL,
  SEOUL_10_DAY,
  SYNTHETIC_8,
];
