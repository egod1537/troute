# troute 알고리즘 성능 벤치마크

측정일: 2026-10-31  
대상 커밋: `b9f8afb97682a4bc7949ef24afa22aa19fa006b4`

## 결론

- `N <= 15`에서는 Exact Bit DP를 우선 사용하는 현재 정책이 타당하다. 모든 소형 품질 케이스에서 선택됐고 brute-force 교차 검증도 전부 통과했다.
- `N = 20`에서는 Simulated Annealing이 8개 품질 케이스 중 6개를 이겼다.
- `N = 30`에서는 SA, Greedy, SCIP가 각각 2회씩 이겨 단일 휴리스틱보다 병렬 orchestrator의 가치가 컸다.
- `N = 50`에서는 Greedy가 8개 중 5개를 이겼다. 즉시 incumbent를 만드는 용도로 가장 유용했다.
- Sparse directed 문제에서는 SCIP가 대형 6개 중 5개를 이겼고 전체 12개를 모두 feasible하게 풀었다. 다만 p50 460.39 ms로 휴리스틱보다 훨씬 느리고, 48회 중 39회가 제한시간에 도달했다.
- Clustered는 선택 횟수는 적지만 전체 성공률 91.7%로 가장 안정적인 범용 휴리스틱이었다.
- MST Double-Tree와 Christofides는 매우 빠르지만 sparse directed에서는 둘 다 0/12 성공이었다. 단독 solver보다는 SA initializer로 쓰는 편이 적합하다.
- Partial Greedy Insertion은 부분 방문 케이스에서 12/12 성공했고 한 번 최종 선택됐다. fallback으로 유지할 가치가 있다.
- LKH-3 실행 파일이 설치되어 있지 않아 이번 측정에서는 제외했다. 실제 수치 없이 기본 활성화하면 안 된다.

## 측정 조건

| 항목 | 값 |
| --- | --- |
| 빌드 | `--release --features solver-mip-scip` |
| Rust | 1.99.1 |
| 타깃 | `x86_64-pc-windows-gnu` |
| CPU | AMD Ryzen 5 9600X, 6코어 12스레드 |
| 메모리 | 32 GB |
| 운영체제 | Windows 11 Pro 64-bit |
| 크기 | N=5, 10, 15, 20, 30, 50 |
| seed | 42, 1337 |
| 품질 시나리오 | directed time windows, sparse directed, zero-minute arcs, partial visit, infeasible |
| 품질 예산 | orchestrator 전체 wall-clock 1초 |
| 병렬도 | 최대 4개 전략, SCIP 1스레드 |

실행 명령:

```powershell
cargo +stable-x86_64-pc-windows-gnu build `
  --release `
  --target x86_64-pc-windows-gnu `
  --features solver-mip-scip `
  --bin algorithm_validation

target/x86_64-pc-windows-gnu/release/algorithm_validation.exe `
  --output-dir benchmark-results/algorithm-comparison-2026-10-31 `
  --n 5,10,15,20,30,50 `
  --seed 42,1337 `
  --budget-seconds 1 `
  --enable-mip
```

Provider 호출을 포함하지 않은 solver-only 벤치마크다. 모든 알고리즘은 동일한 `OptimizationProblem`과 `TravelTimeMatrix`를 사용했다.

## 소형 문제: Exact 기준 품질과 단독 실행시간

각 셀은 두 seed의 `평균 실행시간 / Exact 대비 평균 score 초과`다. score는 동일 방문 범위에서 `travel + wait`이며, 전체 목적함수는 coverage, priority, 시작·종료 시각을 먼저 비교하는 사전식 목적함수다.

| 알고리즘 | N=5 | N=10 | N=15 | Exact 일치 |
| --- | ---: | ---: | ---: | ---: |
| Exact Bit DP | 0.01 ms / +0 | 0.54 ms / +0 | 49.07 ms / +0 | 6/6 |
| Clustered | 0.03 ms / +0 | 0.03 ms / +8.5 | 0.15 ms / +18 | 2/6 |
| Greedy | <0.01 ms / +7 | <0.01 ms / +8.5 | <0.01 ms / +13 | 0/6 |
| MST Double-Tree | 0.01 ms / +8 | <0.01 ms / +28.5 | 0.02 ms / +26 | 0/6 |
| Christofides | 0.01 ms / +7 | 0.01 ms / +25.5 | 0.02 ms / +23.5 | 0/6 |
| Simulated Annealing | 0.79 ms / +0 | 1.30 ms / +1.5 | 3.97 ms / +4 | 2/6 |

Exact DP는 N=5/10의 네 케이스에서 brute-force와 모두 일치했다. N=10 교차 검증 전체 과정은 평균 33.91 ms였으며, frontier pruning은 약 95~96%의 상태를 제거했다.

## 대형 문제: best-known 대비 단독 성능

대형 문제에는 proven optimum이 없으므로 퍼센트 optimality gap을 사용하지 않았다. `score 초과`는 같은 N/seed에서 측정된 최선값과의 절대 차이다.

| 알고리즘 | N=20 | N=30 | N=50 | best-known 승리 |
| --- | ---: | ---: | ---: | ---: |
| Clustered | 2.57 ms / +10.5 | 3.81 ms / +6.5 | 6.22 ms / +12 | 0/6 |
| Greedy | 0.01 ms / +7 | 0.01 ms / +5.5 | 0.01 ms / +0 | 3/6 |
| MST Double-Tree | 0.01 ms / +45 | 0.01 ms / +74.5 | 0.02 ms / +100 | 0/6 |
| Christofides | 0.08 ms / +43 | 4.05 ms / +54 | 0.09 ms / +65.5 | 0/6 |
| Simulated Annealing | 3.74 ms / +0 | 5.32 ms / +0 | 8.49 ms / +12 | 4/6 |

모든 대형 단독 실행은 feasible 경로를 반환했다. N=20/30에서는 SA가 가장 좋았지만 N=50에서는 Greedy가 두 seed 모두 더 좋았다. 따라서 크기만으로 한 휴리스틱을 고정하기보다 Greedy incumbent를 즉시 만들고 남은 예산에서 SA를 실행하는 현재 구조가 합리적이다.

## 제약 시나리오 종합

아래 성공률은 수학적으로 infeasible한 12개 케이스를 제외한 candidate 실행 기준이다. p50/p95는 candidate wall-clock이다.

| 알고리즘 | 시도 | feasible | p50 | p95 | 최종 선택 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Exact Bit DP | 24 | 24 (100%) | 0.31 ms | 50.69 ms | 24 |
| Clustered | 48 | 44 (91.7%) | 0.23 ms | 0.33 ms | 1 |
| Greedy | 48 | 40 (83.3%) | 0.19 ms | 0.28 ms | 8 |
| MST Double-Tree | 48 | 30 (62.5%) | 0.16 ms | 0.26 ms | 0 |
| Christofides | 48 | 32 (66.7%) | 0.14 ms | 2.35 ms | 0 |
| SA 첫 chunk | 18 | 18 (100%) | 0.76 ms | 3.36 ms | SA 전체 9 |
| Partial Greedy Insertion | 12 | 12 (100%) | 0.34 ms | 20.62 ms | 1 |
| SCIP MIP | 48 | 38 (79.2%) | 460.39 ms | 973.17 ms | 5 |

SA 첫 chunk가 18회뿐인 이유는 Exact 또는 병렬 baseline/SCIP가 공통 1초 예산을 먼저 소진한 실행에서는 SA가 시작되지 않았기 때문이다. `최종 선택 9`는 첫 chunk뿐 아니라 뒤의 SA restart와 warm-start 결과까지 합한 수치다.

### 시나리오별 feasible 횟수

| 알고리즘 | Directed windows | Sparse directed | Zero-minute | Partial visit |
| --- | ---: | ---: | ---: | ---: |
| Exact Bit DP | 6/6 | 6/6 | 6/6 | 6/6 |
| Clustered | 12/12 | 8/12 | 12/12 | 12/12 |
| Greedy | 12/12 | 4/12 | 12/12 | 12/12 |
| MST Double-Tree | 10/12 | 0/12 | 10/12 | 10/12 |
| Christofides | 10/12 | 0/12 | 12/12 | 10/12 |
| SCIP MIP | 8/12 | 12/12 | 7/12 | 11/12 |
| Partial Greedy | — | — | — | 12/12 |
| SA 첫 chunk | 5/5 | 4/4 | 4/4 | 5/5 |

Sparse directed에서 MST와 Christofides는 unreachable arc를 피하는 유효 순서를 만들지 못했다. 반면 SCIP는 12/12를 해결해 가장 명확한 장점을 보였다.

## 크기별 최종 선택 분포

각 N에는 4개의 feasible 시나리오와 두 seed, 총 8개 결과가 있다.

| N | 선택 분포 |
| ---: | --- |
| 5 | Exact 8 |
| 10 | Exact 8 |
| 15 | Exact 8 |
| 20 | SA 6, SCIP 1, Greedy 1 |
| 30 | SA 2, SCIP 2, Greedy 2, Clustered 1, Partial Greedy 1 |
| 50 | Greedy 5, SCIP 2, SA 1 |

Orchestrator는 feasible 시나리오 48/48에서 경로를 반환했고, 의도적으로 infeasible한 12/12는 모두 실패로 판정했다. 최초 feasible 발견시간은 시나리오별 p50 0.11~0.23 ms, p95 0.20~0.67 ms였다.

## SCIP 상세

SCIP timeout은 반드시 실패를 뜻하지 않는다. 제한시간에 도달해도 incumbent가 있으면 feasible 후보로 사용된다.

| N | feasible | timeout | p50 | p95 | 최종 선택 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 5 | 8/8 | 0 | 22.80 ms | 32.55 ms | 0 |
| 10 | 8/8 | 7 | 308.76 ms | 777.33 ms | 0 |
| 15 | 8/8 | 8 | 341.36 ms | 829.55 ms | 0 |
| 20 | 7/8 | 8 | 513.91 ms | 971.54 ms | 1 |
| 30 | 3/8 | 8 | 565.87 ms | 986.01 ms | 2 |
| 50 | 4/8 | 8 | 538.93 ms | 973.17 ms | 2 |

48회 중 9회는 optimal proof, 29회는 timeout 시점의 feasible incumbent, 10회는 feasible 경로 없이 종료됐다. 따라서 SCIP는 모든 문제에 기본 우선순위를 줄 정도로 빠르지는 않지만 sparse/unreachable arc가 많은 문제에서는 계속 병렬 후보로 실행할 가치가 있다.

## 운영 권장안

1. `N <= 15`: Exact DP 우선 실행 후 optimal이면 즉시 종료한다.
2. `N = 16~25`: Greedy/Clustered로 즉시 incumbent를 만들고 SA에 대부분의 잔여 예산을 준다.
3. `N >= 30`: Greedy를 반드시 포함한다. N=50에서 가장 안정적인 최종 선택이었다.
4. Sparse directed: SCIP를 활성화한다. MST/Christofides 단독 결과에 의존하지 않는다.
5. Partial visit: Partial Greedy를 fallback으로 유지하고 SA/SCIP와 함께 비교한다.
6. MST/Christofides: 단독 solver보다 SA initializer 및 후보 다양화 용도로 사용한다.
7. LKH-3: 설치 후 같은 입력·1초 예산으로 다시 측정하기 전에는 기본 활성화하지 않는다.

## 한계와 후속 측정

- seed가 두 개뿐이므로 신뢰구간을 주장할 수 없다. 회귀 게이트에는 seed를 최소 10개로 늘려야 한다.
- 실제 서울 fixture나 provider latency는 포함하지 않았다. 이 결과는 solver 알고리즘 비교이며 API E2E 성능이 아니다.
- CPU time과 peak RSS는 측정하지 않았다. wall-clock과 solver thread 설정만 기록됐다.
- 1초 예산 결과이므로 5초/30초에서 SCIP와 SA의 상대 순위가 달라질 수 있다.
- LKH-3는 외부 실행 파일 부재로 미측정이다.
- legacy `orchestrator-results.json` 생성 경로는 CLI의 `--budget-seconds` 대신 내부 기본 30초를 사용하고 모든 SA restart를 기록해 약 47 MB가 됐다. 이번 결론은 공통 1초 예산을 실제 적용한 `solver-quality-results.json`을 기준으로 했으며, CI 도입 전 legacy runner의 budget 연결과 SA diagnostics 집계가 필요하다.

## 원시 결과

- [실행 manifest](../benchmark-results/algorithm-comparison-2026-10-31/benchmark-manifest.json)
- [Exact와 brute-force 교차 검증](../benchmark-results/algorithm-comparison-2026-10-31/exact-vs-bruteforce.json)
- [소형 알고리즘 품질](../benchmark-results/algorithm-comparison-2026-10-31/heuristic-gap.json)
- [대형 알고리즘 성능](../benchmark-results/algorithm-comparison-2026-10-31/large-n-performance.json)
- [공통 1초 예산 품질 결과](../benchmark-results/algorithm-comparison-2026-10-31/solver-quality-results.json)

