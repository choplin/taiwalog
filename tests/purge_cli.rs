use std::{fs, process::Command};

use tempfile::TempDir;

fn agentlog(home: &std::path::Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agentlog"))
        .arg("--home")
        .arg(home)
        .args(arguments)
        .output()
        .expect("run agentlog process")
}

#[test]
fn purge_reports_to_stderr_and_rejects_legacy_forms() {
    let temporary = TempDir::new_in("/private/tmp").expect("temporary directory");
    let home = temporary.path().join("agentlog");
    fs::create_dir(&home).expect("create Agentlog home");

    let preview = agentlog(&home, &["purge"]);
    assert!(preview.status.success());
    assert!(
        preview.stdout.is_empty(),
        "purge does not have a product output"
    );
    let preview_report = String::from_utf8(preview.stderr).expect("UTF-8 preview report");
    assert!(preview_report.contains("Agentlog-owned database target:"));
    assert!(preview_report.contains("Catalog sessions: 0"));
    assert!(preview_report.contains("Transcript items: 0"));
    assert!(preview_report.contains("Approximate database size: 0 B (0 bytes)"));
    assert!(preview_report.contains("No changes were made."));
    assert!(preview_report.contains("agentlog purge --yes"));
    assert!(preview_report.contains(
        "Catalog data may not be reconstructible: only provider logs that still exist and remain readable can be synchronized."
    ));

    let confirmed = agentlog(&home, &["purge", "--yes"]);
    assert!(confirmed.status.success());
    assert!(confirmed.stdout.is_empty());
    let completion_report = String::from_utf8(confirmed.stderr).expect("UTF-8 completion report");
    assert!(completion_report.contains("Purge complete:"));
    assert!(completion_report.contains(
        "Catalog data may not be reconstructible: only provider logs that still exist and remain readable can be synchronized."
    ));

    assert!(!agentlog(&home, &["data", "clear"]).status.success());
    assert!(!agentlog(&home, &["purge", "--dry-run"]).status.success());
}
