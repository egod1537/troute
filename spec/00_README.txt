troute / impl 브랜치 — Top-down 가독성 리팩토링 명세 패키지
========================================================

목적
- 외부 진입점(public API, HTTP Handler, main, Solver의 solve 등)을 먼저 읽고,
  그 아래에 조립 로직, 세부 구현, 헬퍼를 따라 내려갈 수 있도록 소스 구성을 정돈한다.
- '위에서 아래로 읽히는 코드'가 목적이지 모든 로직을 객체로 만드는 것이 목적이 아니다.
- 기존의 테스트 분리, Server/Bootstrap 분리 작업은 보존한다.

기준
- Repository: https://github.com/egod1537/troute
- 작업 브랜치: impl
- 명세 작성 시 확인한 HEAD: ea1f201ae5af79e71e13589188a7a4bb5a161727
- 실제 작업 직전에 impl 최신 HEAD를 다시 확인하고, 변경된 소스에 맞춰 적용한다.
- 대상: Rust src/ (필요시 기존 tests/의 참조 확인). README, 프런트엔드, 알고리즘 동작은 작업 대상 아님.

작업 파일 / 순서
1. 01_GLOBAL_RULES.txt: 모든 단계에 공통 적용할 설계 규칙
2. 02_HTTP_TOPDOWN.txt: HTTP 진입점과 Router
3. 03_SERVER_TOPDOWN.txt: Server, Bootstrap, 설정 주입
4. 04_SOLVER_TOPDOWN.txt: Solver, Orchestrator, 알고리즘
5. 05_OTHER_MODULES.txt: Service, Jobs, Storage, API, Routing, Config, Bin 등
6. 06_VALIDATION.txt: 각 단계 검증 및 완료 기준
7. 07_CODEX_INSTRUCTIONS.txt: Codex에 전달할 실행 지시문

적용 방식
- 각 단계는 독립적으로 처리하고 검증한다. 한 번에 전체 파일을 기계적으로 재배열하지 않는다.
- 파일별로 먼저 외부 API/대표 실행 흐름/내부 헬퍼를 식별한다.
- 이미 Top-down인 파일은 변경하지 않는다. 작은 함수까지 의미 없이 분리하지 않는다.
- 각 단계의 변경 내역을 리뷰 가능한 크기로 유지한다.
- 코드를 이동한 이유를 최종 결과 보고서에 간략하게 남긴다.

절대 금지
- 비즈니스 로직, 알고리즘, 에러 의미/우선순위, 실행 순서, 공개 API를 바꾸지 않는다.
- 새 DI 컨테이너, Rx/Reactive 체인, 비필요한 trait/struct, 과도한 파일 분할 도입 금지.
- 테스트를 다시 인라인으로 집어넣거나, Rust 단위 테스트를 통합 테스트로 이동하지 않는다.
- mod.rs를 새로 만들지 않는다.
