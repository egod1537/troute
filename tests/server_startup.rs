use std::process::Command;

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
fn missing_tcache_setting_preserves_error_prefix_and_failure_exit_code() {
    let output = run_with_environment("8080", None);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(
        "troute server error: invalid tcache configuration: TCACHE_BASE_URL is required"
    ));
}
