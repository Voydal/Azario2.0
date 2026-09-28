use std::{fs, process::Command};

use tempfile::tempdir;

#[test]
fn fixture_evaluate_and_sweep_create_reports() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/perception-eval/dataset.toml");
    let output = tempdir().unwrap();
    let first = output.path().join("evaluate");
    let status = Command::new(env!("CARGO_BIN_EXE_perception-evaluator"))
        .args(["evaluate", "--dataset"])
        .arg(&root)
        .args(["--output"])
        .arg(&first)
        .status()
        .unwrap();
    assert!(status.success());
    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(first.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["report_schema_version"], 1);
    assert_eq!(summary["number_of_spots"], 2);
    assert_eq!(summary["prediction_sample_count"], 24);
    assert!(summary["single_evaluation"]["frame"].is_object());
    assert!(summary["single_evaluation"]["stabilized"].is_object());
    assert!(
        fs::read_to_string(first.join("results.csv"))
            .unwrap()
            .contains("false_free_rate")
    );
    assert!(first.join("evaluation-config.json").exists());
    assert!(first.join("per_camera.csv").exists());
    assert!(
        fs::read_to_string(first.join("results.csv"))
            .unwrap()
            .contains("frame_macro_f1")
    );

    let second = output.path().join("sweep");
    let status = Command::new(env!("CARGO_BIN_EXE_perception-evaluator"))
        .args(["sweep", "--dataset"])
        .arg(&root)
        .args(["--output"])
        .arg(&second)
        .args([
            "--confidence-values",
            "0.4,0.5",
            "--free-values",
            "0.1,0.3",
            "--occupied-values",
            "0.3",
            "--stable-samples-values",
            "1,2",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(second.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["configurations_evaluated"], 4);
    assert_eq!(summary["invalid_combinations_skipped"], 4);
    assert!(summary["selected_configuration"].is_null());
    let repeat = output.path().join("repeat");
    let status = Command::new(env!("CARGO_BIN_EXE_perception-evaluator"))
        .args(["evaluate", "--dataset"])
        .arg(&root)
        .args(["--output"])
        .arg(&repeat)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        fs::read(first.join("summary.json")).unwrap(),
        fs::read(repeat.join("summary.json")).unwrap()
    );
    assert_eq!(
        fs::read(first.join("results.csv")).unwrap(),
        fs::read(repeat.join("results.csv")).unwrap()
    );
}
