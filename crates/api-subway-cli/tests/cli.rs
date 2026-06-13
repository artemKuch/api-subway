use std::{fs, path::PathBuf, process::Command};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures")
        .join(name)
}

#[test]
fn generate_and_check_follow_the_documented_exit_contract() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let output = temporary.path().join("docs/api-subway");
    let binary = env!("CARGO_BIN_EXE_api-subway");
    let generated = Command::new(binary)
        .args(["generate", fixture("next").to_str().expect("fixture path")])
        .args(["--framework", "next", "--format", "json", "--out"])
        .arg(&output)
        .status()
        .expect("generate command");
    assert_eq!(generated.code(), Some(0));

    let current = Command::new(binary)
        .args(["check", fixture("next").to_str().expect("fixture path")])
        .args(["--framework", "next", "--format", "json", "--out"])
        .arg(&output)
        .status()
        .expect("check command");
    assert_eq!(current.code(), Some(0));

    fs::write(output.with_extension("json"), "stale\n").expect("mutate generated artifact");
    let stale = Command::new(binary)
        .args(["check", fixture("next").to_str().expect("fixture path")])
        .args(["--framework", "next", "--format", "json", "--out"])
        .arg(&output)
        .status()
        .expect("stale check command");
    assert_eq!(stale.code(), Some(1));
}

#[test]
fn strict_diagnostics_return_two_without_writing() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    fs::write(
        temporary.path().join("package.json"),
        r#"{"name":"dynamic-api","dependencies":{"express":"latest"}}"#,
    )
    .expect("package fixture");
    fs::write(
        temporary.path().join("app.ts"),
        "import express from 'express';\nconst app = express();\napp.get(base + '/users', handler);\n",
    )
    .expect("source fixture");
    let output = temporary.path().join("generated/map");
    let status = Command::new(env!("CARGO_BIN_EXE_api-subway"))
        .arg("generate")
        .arg(temporary.path())
        .args([
            "--framework",
            "express",
            "--format",
            "json",
            "--strict",
            "--out",
        ])
        .arg(&output)
        .status()
        .expect("strict command");
    assert_eq!(status.code(), Some(2));
    assert!(!output.with_extension("json").exists());
}

#[test]
fn strict_empty_analysis_returns_two_without_writing() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    fs::write(
        temporary.path().join("index.ts"),
        "export const value = 42;\n",
    )
    .expect("source fixture");
    let output = temporary.path().join("generated/map");
    let status = Command::new(env!("CARGO_BIN_EXE_api-subway"))
        .arg("generate")
        .arg(temporary.path())
        .args(["--format", "json", "--strict", "--out"])
        .arg(&output)
        .status()
        .expect("strict command");

    assert_eq!(status.code(), Some(2));
    assert!(!output.with_extension("json").exists());
}

#[test]
fn rejects_an_empty_output_basename() {
    let status = Command::new(env!("CARGO_BIN_EXE_api-subway"))
        .args([
            "generate",
            fixture("next").to_str().expect("fixture path"),
            "--out",
            "",
        ])
        .status()
        .expect("generate command");
    assert_eq!(status.code(), Some(2));
}
