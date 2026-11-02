use std::fs;

use tempfile::tempdir;

use super::StorageRuntime;

#[test]
fn initialization_rejects_a_data_path_that_is_a_file() {
    let root = tempdir().unwrap();
    let data_file = root.path().join("not-a-directory");
    fs::write(&data_file, b"occupied").unwrap();

    let error = match StorageRuntime::initialize(&data_file) {
        Ok(_) => panic!("a file cannot be used as the storage data directory"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("job storage I/O failed"));
}
