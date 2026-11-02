# Solver quality benchmark

Selected feasible results at the 5-second wall-clock budget are shown below. Lexicographic objective components are reported separately; no percentage gap is invented when no proven reference exists.

| scenario | N | seed | strategy | coverage | priority | start | finish | travel | wait |
| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| DirectedTimeWindows | 5 | 42 | exact_bit_dp | 0 | 0 | 480 | 630 | 89 | 21 |
| SparseDirected | 5 | 42 | exact_bit_dp | 0 | 0 | 480 | 520 | 10 | 0 |
| ZeroMinuteArcs | 5 | 42 | exact_bit_dp | 0 | 0 | 480 | 547 | 37 | 0 |
| PartialVisit | 5 | 42 | exact_bit_dp | 3 | 231 | 480 | 583 | 73 | 0 |

SCIP and LKH-3 rows appear only when explicitly enabled. LKH-3 remains an externally installed, non-redistributed executable. CPU time and peak RSS should be collected by the surrounding OS benchmark harness.
