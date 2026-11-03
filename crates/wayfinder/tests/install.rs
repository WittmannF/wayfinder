//! Harness installation: the configuration a client reads and the instructions
//! a model reads, merged rather than replaced.

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// Run the binary in one project directory with a private home directory.
fn wayfinder(project: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wayfinder"))
        .args(args)
        .current_dir(project)
        .env("HOME", home)
        .output()
        .expect("run wayfinder")
}

/// One project with an initialised pool and a private home.
fn project() -> (tempfile::TempDir, tempfile::TempDir) {
    let project = tempfile::tempdir().expect("project");
    let home = tempfile::tempdir().expect("home");
    let init = wayfinder(project.path(), home.path(), &["init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    (project, home)
}

/// Read one JSON file.
fn json_at(path: &Path) -> Value {
    let text =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("valid json")
}

#[test]
fn claude_gets_an_mcp_entry_and_an_instruction_block() {
    let (project, home) = project();
    let output = wayfinder(project.path(), home.path(), &["install", "claude"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let config = json_at(&project.path().join(".mcp.json"));
    assert_eq!(config["mcpServers"]["wayfinder"]["command"], "wayfinder");
    assert_eq!(config["mcpServers"]["wayfinder"]["args"][0], "mcp");

    let instructions = fs::read_to_string(project.path().join("CLAUDE.md")).expect("CLAUDE.md");
    assert!(instructions.contains("<!-- wayfinder:begin -->"));
    assert!(instructions.contains("Workspace memory (wayfinder)"));
    assert!(instructions.contains("`memory_search`"));
    assert!(instructions.contains("`memory_save`"));
    assert!(instructions.ends_with("<!-- wayfinder:end -->\n"));
}

#[test]
fn a_second_install_changes_nothing() {
    let (project, home) = project();
    assert!(
        wayfinder(project.path(), home.path(), &["install", "claude"])
            .status
            .success()
    );
    let first_config = fs::read_to_string(project.path().join(".mcp.json")).expect("config");
    let first_instructions =
        fs::read_to_string(project.path().join("CLAUDE.md")).expect("instructions");

    let again = wayfinder(project.path(), home.path(), &["install", "claude"]);
    assert!(again.status.success());
    let report = String::from_utf8_lossy(&again.stdout);
    assert!(report.contains("left"), "{report}");
    assert_eq!(
        fs::read_to_string(project.path().join(".mcp.json")).expect("config"),
        first_config
    );
    assert_eq!(
        fs::read_to_string(project.path().join("CLAUDE.md")).expect("instructions"),
        first_instructions
    );
    // The block appears exactly once, whatever the number of installs.
    assert_eq!(
        first_instructions
            .matches("<!-- wayfinder:begin -->")
            .count(),
        1
    );
}

#[test]
fn an_existing_entry_is_left_alone_until_force() {
    let (project, home) = project();
    fs::write(
        project.path().join(".mcp.json"),
        r#"{"mcpServers":{"wayfinder":{"command":"other-binary","args":["serve"]},"keep":{"command":"keep"}}}"#,
    )
    .expect("pre-existing config");

    let refused = wayfinder(project.path(), home.path(), &["install", "claude"]);
    assert!(refused.status.success());
    let report = String::from_utf8_lossy(&refused.stdout);
    assert!(report.contains("already exists"), "{report}");
    let config = json_at(&project.path().join(".mcp.json"));
    assert_eq!(config["mcpServers"]["wayfinder"]["command"], "other-binary");
    assert_eq!(config["mcpServers"]["keep"]["command"], "keep");

    let forced = wayfinder(
        project.path(),
        home.path(),
        &["install", "claude", "--force"],
    );
    assert!(forced.status.success());
    let config = json_at(&project.path().join(".mcp.json"));
    assert_eq!(config["mcpServers"]["wayfinder"]["command"], "wayfinder");
    // The forced install keeps the harness's other servers.
    assert_eq!(config["mcpServers"]["keep"]["command"], "keep");
}

#[test]
fn existing_instructions_keep_their_prose_around_the_block() {
    let (project, home) = project();
    fs::write(
        project.path().join("CLAUDE.md"),
        "# House rules\n\nBe brief.\n",
    )
    .expect("preamble");
    assert!(
        wayfinder(project.path(), home.path(), &["install", "claude"])
            .status
            .success()
    );
    let instructions = fs::read_to_string(project.path().join("CLAUDE.md")).expect("instructions");
    assert!(instructions.starts_with("# House rules"));
    assert!(instructions.contains("Be brief."));
    assert!(instructions.contains("Workspace memory (wayfinder)"));

    // A second run replaces the block instead of appending another copy.
    assert!(
        wayfinder(
            project.path(),
            home.path(),
            &["install", "claude", "--force"]
        )
        .status
        .success()
    );
    let instructions = fs::read_to_string(project.path().join("CLAUDE.md")).expect("instructions");
    assert_eq!(instructions.matches("<!-- wayfinder:begin -->").count(), 1);
    assert!(instructions.starts_with("# House rules"));
}

#[test]
fn copilot_cursor_and_gemini_get_their_own_shapes() {
    let (project, home) = project();
    assert!(
        wayfinder(project.path(), home.path(), &["install", "copilot"])
            .status
            .success()
    );
    let vscode = json_at(&project.path().join(".vscode/mcp.json"));
    assert_eq!(vscode["servers"]["wayfinder"]["type"], "stdio");
    assert_eq!(vscode["servers"]["wayfinder"]["args"][0], "mcp");
    assert!(
        project
            .path()
            .join(".github/copilot-instructions.md")
            .is_file()
    );

    assert!(
        wayfinder(project.path(), home.path(), &["install", "cursor"])
            .status
            .success()
    );
    let cursor = json_at(&project.path().join(".cursor/mcp.json"));
    assert_eq!(cursor["mcpServers"]["wayfinder"]["command"], "wayfinder");
    assert!(project.path().join(".cursor/rules/wayfinder.mdc").is_file());

    assert!(
        wayfinder(project.path(), home.path(), &["install", "gemini"])
            .status
            .success()
    );
    let gemini = json_at(&project.path().join(".gemini/settings.json"));
    assert_eq!(gemini["mcpServers"]["wayfinder"]["command"], "wayfinder");
    assert!(project.path().join("GEMINI.md").is_file());
}

#[test]
fn codex_gets_a_toml_table_in_the_home_configuration() {
    let (project, home) = project();
    assert!(
        wayfinder(project.path(), home.path(), &["install", "codex"])
            .status
            .success()
    );
    let config = fs::read_to_string(home.path().join(".codex/config.toml")).expect("config");
    assert!(config.contains("[mcp_servers.wayfinder]"), "{config}");
    assert!(config.contains("command = \"wayfinder\""), "{config}");
    assert!(config.contains("args = [\"mcp\"]"), "{config}");
    assert!(project.path().join("AGENTS.md").is_file());

    // A user's own configuration keeps its other tables.
    let second_project = tempfile::tempdir().expect("project");
    let second_home = tempfile::tempdir().expect("home");
    assert!(
        wayfinder(second_project.path(), second_home.path(), &["init"])
            .status
            .success()
    );
    fs::create_dir_all(second_home.path().join(".codex")).expect("codex dir");
    fs::write(
        second_home.path().join(".codex/config.toml"),
        "[model]\nname = \"qwen3\"\n",
    )
    .expect("preexisting");
    assert!(
        wayfinder(
            second_project.path(),
            second_home.path(),
            &["install", "codex"]
        )
        .status
        .success()
    );
    let config = fs::read_to_string(second_home.path().join(".codex/config.toml")).expect("config");
    assert!(config.contains("[model]"), "{config}");
    assert!(config.contains("[mcp_servers.wayfinder]"), "{config}");
}

#[test]
fn the_generic_harness_gets_commands_not_tools() {
    let (project, home) = project();
    assert!(
        wayfinder(project.path(), home.path(), &["install", "agents"])
            .status
            .success()
    );
    let instructions = fs::read_to_string(project.path().join("AGENTS.md")).expect("AGENTS.md");
    assert!(
        instructions.contains("`wayfinder search \"<question>\"`"),
        "{instructions}"
    );
    assert!(!instructions.contains("`memory_search`"), "{instructions}");
    assert!(!project.path().join(".mcp.json").exists());
}
