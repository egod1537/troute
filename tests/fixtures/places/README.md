# Real Place ID fixtures

이 디렉터리는 troute unit test, 수동 tcache integration test, tcache Matrix
Testbed가 함께 사용하는 Google Place ID의 source of truth다. troute는 이
데이터를 사용할 뿐 Places API를 호출하거나 런타임에 장소를 검색하지 않는다.

## Fixture 구성

- `tokyo_3_places.json`, `tokyo_5_places.json`, `tokyo_10_places.json`
- `seoul_3_places.json`, `seoul_5_places.json`, `seoul_10_places.json`

각 도시의 3/5개 fixture는 10개 fixture의 순서가 같은 prefix다. `id`는 로그와
결과 매핑에 쓰는 안정적인 slug이며 `place_id`와 분리되어 있다.

## 출처와 검증 범위

- 마지막 Place ID 조회: 2026-09-22
- 출처: Google Places API (New)의 Text Search가 반환한 canonical Place ID
- 요청 필드: place ID, display name, formatted address, location, primary type
- 지역 확인: 반환된 주소와 좌표가 fixture의 도시/국가에 속하는지 확인
- 실제 경로 확인: 로컬 tcache의 기존 Route Job API와 Google provider를 통해
  Tokyo Station → Shibuya Station `WALKING` 요청이 완료됨
  (`route_3f5bb21825ac4d259cbc2ddaf218372c`, cache miss, 5965 seconds)

Place ID 조회 성공과 모든 mode/pair에서 경로가 존재한다는 보장은 서로 다르다.
예를 들어 provider 정책이나 대중교통 데이터에 따라 특정 pair/mode가 route
unavailable일 수 있다. 전체 matrix 검증은 아래 ignored integration test가 담당한다.

Google은 Place ID를 저장해 재사용할 수 있지만 변경될 수 있다고 설명하며, 오래된
ID는 갱신할 것을 권장한다. 참고:

- https://developers.google.com/maps/documentation/places/web-service/place-id
- https://developers.google.com/maps/documentation/places/web-service/text-search

## 기본 검증

기본 검증은 네트워크나 Google API 비용을 사용하지 않는다.

```sh
./scripts/verify_place_fixtures.sh
```

검증 항목은 JSON/schema 파싱, 필수 필드, region/timezone, 중복 `id`, 중복
`place_id`, 선언된 location 수, 3/5/10 subset 순서, troute
`OptimizationProblem` 변환이다. 일반 `cargo test`에도 같은 테스트가 포함된다.

## 실제 tcache integration test

matrix endpoint가 포함된 tcache와 Google Routes provider를 실행한 뒤 명시적으로
ignored test를 실행한다. 기본 CI에서는 실행하지 않는다.

```sh
TCACHE_BASE_URL=http://localhost:3200 \
RUN_REAL_ROUTE_TESTS=1 \
./scripts/verify_place_fixtures.sh
```

또는:

```sh
TCACHE_BASE_URL=http://localhost:3200 \
cargo test --test tcache_integration -- --ignored --nocapture
```

실패 메시지에는 fixture 이름, location slug/name/Place ID, 실패 단계와 tcache
오류가 포함된다. API key와 authorization header는 출력하지 않는다.

## Testbed preset 생성

tcache Testbed용 TypeScript 파일은 JSON fixture에서 생성된 산출물이다. 두 저장소가
같은 상위 디렉터리에 있을 때 다음을 실행한다.

```sh
node scripts/generate_testbed_place_presets.mjs
node scripts/generate_testbed_place_presets.mjs --check
```

다른 위치라면 `TCACHE_REPOSITORY`에 troute 기준 상대 경로 또는 절대 경로를 지정한다.
생성된 `place-fixtures.generated.ts`는 직접 수정하지 않는다.

## 장소 추가/갱신

1. Google Places API (New)에서 정확한 장소를 조회하고 이름, 주소, 좌표를 확인한다.
2. 해당 도시의 10개 fixture를 먼저 수정한다.
3. 3/5 fixture가 10개 fixture의 prefix가 되도록 갱신한다.
4. `verified_at`을 실제 확인일로 바꾼다.
5. 기본 검증과 Testbed preset 생성을 실행한다.
6. 실제 tcache route 또는 matrix query로 최소 한 번 확인한다.

Place ID invalid가 확인된 경우에만 값을 교체하고, 변경 설명에는 장소명, 이전 ID,
새 ID, 검증일을 남긴다. API key나 provider secret은 fixture에 저장하지 않는다.
