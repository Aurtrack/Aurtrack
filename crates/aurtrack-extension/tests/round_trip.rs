use std::path::PathBuf;

use aurtrack_extension::{load_extension, run_extension_command};
use serde_json::json;

#[test]
fn extension_process_round_trip() {
    let directory = PathBuf::from("target/extension-e2e");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let executable_name = if cfg!(windows) { "hello.exe" } else { "hello" };
    let executable = directory.join(executable_name);
    std::fs::copy(env!("CARGO_BIN_EXE_extension_echo"), &executable).unwrap();
    std::fs::write(
        directory.join("manifest.json"),
        format!(
            r#"{{
                "id": "com.example.echo",
                "name": "Echo",
                "version": "0.1.0",
                "api_version": 1,
                "commands": [
                    {{ "id": "echo", "name": "Echo", "executable": "{executable_name}" }}
                ]
            }}"#
        ),
    )
    .unwrap();

    let extension = load_extension(&directory).unwrap();
    let result = run_extension_command(&extension, "echo", &json!({ "value": 42 })).unwrap();
    assert_eq!(result["echo"]["value"], 42);
    assert_eq!(result["extension"], "com.example.echo");
}
