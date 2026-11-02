use std::process::Command;

#[test]
fn portfolio_benchmark_smoke_writes_versioned_artifacts_without_provider_calls() {
    let output = tempfile::tempdir().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_portfolio_benchmark"))
        .args([
            "--output-dir",
            output.path().to_str().unwrap(),
            "--policies",
            "CURRENT,PARALLEL_ALL",
            "--seeds",
            "1",
            "--sizes",
            "5",
            "--scenarios",
            "wide,required-infeasible",
            "--budgets-ms",
            "10",
            "--slots",
            "1",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let experiment_dirs: Vec<_> = std::fs::read_dir(output.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    assert_eq!(experiment_dirs.len(), 1);
    for name in [
        "manifest.json",
        "runs.jsonl",
        "summary.json",
        "summary.csv",
        "timelines.jsonl",
    ] {
        assert!(experiment_dirs[0].join(name).is_file(), "missing {name}");
    }
    let runs = std::fs::read_to_string(experiment_dirs[0].join("runs.jsonl")).unwrap();
    let records: Vec<serde_json::Value> = runs
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records.iter().any(|record| {
        record["scenario"] == "required-infeasible"
            && record["feasible"] == false
            && record["status"] == "FAILED"
    }));
}
