use std::{
    net::{TcpListener, TcpStream},
    process::Command,
    thread,
    time::Duration,
};

fn run_with_environment(port: &str, tcache_base_url: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_troute"));
    command.env("TROUTE_PORT", port);
    match tcache_base_url {
        Some(value) => {
            command.env("TCACHE_BASE_URL", value);
        }
        None => {
            command.env_remove("TCACHE_BASE_URL");
        }
    }
    command.output().expect("troute child process must start")
}

#[test]
fn invalid_port_preserves_error_prefix_and_failure_exit_code() {
    let output = run_with_environment("0", None);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("troute server error: TROUTE_PORT must be an integer between 1 and 65535")
    );
}

#[test]
fn server_optimizes_a_supplied_matrix_without_tcache_configuration() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let data_dir = tempfile::tempdir().unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_troute"));
    command
        .env("TROUTE_PORT", port.to_string())
        .env("TROUTE_DATA_DIR", data_dir.path())
        .env_remove("TCACHE_BASE_URL");
    let mut child = command.spawn().expect("troute child process must start");

    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("troute exited before listening without tcache: {status}");
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let supplied_matrix_response = client
                .post(format!("http://127.0.0.1:{port}/optimize"))
                .header("content-type", "application/json")
                .body(include_str!("../docs/examples/basic-request.json"))
                .send()
                .and_then(|response| {
                    let status = response.status();
                    response.text().map(|body| (status, body))
                });

            let mut routed_request: serde_json::Value =
                serde_json::from_str(include_str!("../docs/examples/basic-request.json")).unwrap();
            routed_request["job_id"] = serde_json::json!("missing-tcache-example");
            routed_request["travel_time_matrix"] = serde_json::Value::Null;
            for (index, location) in routed_request["locations"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                location["place_id"] = serde_json::json!(format!("place-{index}"));
            }
            let routed_response = client
                .post(format!("http://127.0.0.1:{port}/optimize"))
                .header("content-type", "application/json")
                .body(routed_request.to_string())
                .send()
                .and_then(|response| {
                    let status = response.status();
                    response.text().map(|body| (status, body))
                });

            child.kill().unwrap();
            child.wait().unwrap();
            let (status, body) =
                supplied_matrix_response.expect("matrix optimization request must complete");
            assert_eq!(status, reqwest::StatusCode::OK, "{body}");
            let body: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["total_travel_minutes"], 35);

            let (status, body) =
                routed_response.expect("unconfigured routing request must complete");
            assert_eq!(status, reqwest::StatusCode::SERVICE_UNAVAILABLE, "{body}");
            let body: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["error"]["code"], "PROVIDER_NOT_CONFIGURED");
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    child.kill().unwrap();
    child.wait().unwrap();
    panic!("troute did not listen without tcache configuration");
}
