//! Exercise the shipped executable from a consumer directory, without a TTY.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lobo-agent-{}",
            lobo_primitives::uuid::Uuid::new_v4()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lobo"))
            .args(args)
            .current_dir(&self.0)
            .env_remove("LOBO_SYMBOL")
            .env_remove("LOBO_ITCH_PATH")
            .output()
            .unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn offline_discovery_exposes_nested_commands_docs_and_wire_schemas() {
    let scratch = Scratch::new();
    let catalog = scratch.json(&["completions", "api"]);
    assert_eq!(catalog["schema_version"], 1);
    let commands = catalog["cli"]["subcommands"].as_array().unwrap();
    for name in [
        "dashboard",
        "session",
        "ctl",
        "completions",
        "docs",
        "api",
        "skill",
        "presets",
    ] {
        assert!(commands.iter().any(|c| c["name"] == name), "missing {name}");
    }
    let api = commands.iter().find(|c| c["name"] == "api").unwrap();
    assert!(
        api["subcommands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "schema")
    );
    assert_eq!(catalog, scratch.json(&["completion", "api"]));
    assert_eq!(catalog["apis"], scratch.json(&["api", "schema", "--json"]));
    assert!(
        scratch
            .run(&["api", "schema", "--output", "schema.json"])
            .status
            .success()
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(scratch.0.join("schema.json")).unwrap()).unwrap();
    assert_eq!(catalog["apis"], saved);
    let docs = scratch.json(&["docs", "all", "--json"]);
    assert_eq!(
        docs.as_array().unwrap().len(),
        catalog["documentation"].as_array().unwrap().len()
    );
    for doc in docs.as_array().unwrap() {
        let topic = doc["id"].as_str().unwrap();
        assert!(!doc["content"].as_str().unwrap().is_empty());
        assert_eq!(&scratch.json(&["docs", topic, "--json"])[0], doc);
    }
    assert!(!scratch.run(&["docs", "nonexistent-topic"]).status.success());
    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        let result = scratch.run(&["completions", shell]);
        assert!(result.status.success());
        assert!(!result.stdout.is_empty());
    }
    let help = scratch.run(&["--help"]);
    assert!(String::from_utf8_lossy(&help.stdout).contains("completions api"));
}

#[test]
fn every_offline_preset_runs_and_user_options_win() {
    let scratch = Scratch::new();
    let presets = scratch.json(&["presets", "--json"]);
    assert_eq!(presets.as_array().unwrap().len(), 10);
    for preset in presets.as_array().unwrap() {
        let name = preset["name"].as_str().unwrap();
        // Override live sources to exercise every preset deterministically.
        let state = scratch.json(&[
            "--preset",
            name,
            "--source",
            "demo",
            "--snapshot-json",
            "--renderer",
            "cpu",
            "--capture-seconds",
            "0.1",
        ]);
        assert_eq!(state["symbol"], "AAPL");
        assert!(state["levels"].as_array().is_some_and(|a| !a.is_empty()));
        if name == "simulator" {
            assert!(state["simulation"].is_object());
        }
    }
    // A view override must suppress the simulator preset's automatic preview.
    let state = scratch.json(&[
        "--preset",
        "simulator",
        "book",
        "--snapshot-json",
        "--renderer",
        "cpu",
        "--capture-seconds",
        "0.1",
    ]);
    assert!(state["simulation"].is_null());
    // Options after the command override preset defaults too.
    let state = scratch.json(&[
        "--preset",
        "tick-bars",
        "candles",
        "--bar-size",
        "1",
        "--speed",
        "100",
        "--snapshot-json",
        "--renderer",
        "cpu",
        "--capture-seconds",
        "0.2",
    ]);
    let complete: Vec<_> = state["candles"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["forming"] == false)
        .collect();
    assert!(!complete.is_empty());
    assert!(complete.iter().all(|c| c["ticks"] == 1));
    assert!(
        !scratch
            .run(&["--preset", "tick-bars", "--bar-size", "0"])
            .status
            .success()
    );
}

#[test]
fn skill_installs_offline_with_docs_and_preserves_local_changes() {
    let scratch = Scratch::new();
    let installed = scratch.json(&["skill", "install"]);
    assert_eq!(installed["skill"], "lobo-terminal");
    let root = scratch.0.join(".agents/skills/lobo-terminal");
    let skill = fs::read(root.join("SKILL.md")).unwrap();
    assert_eq!(skill, scratch.run(&["--skill"]).stdout);
    assert_eq!(skill, scratch.run(&["skills", "show"]).stdout);
    let docs = scratch.json(&["docs", "all", "--json"]);
    for doc in docs.as_array().unwrap() {
        assert_eq!(
            fs::read_to_string(root.join(doc["resource"].as_str().unwrap())).unwrap(),
            doc["content"]
        );
    }
    assert!(root.join("references/python-api").is_dir());
    assert!(scratch.run(&["skill", "install"]).status.success());
    fs::write(root.join("SKILL.md"), "user edits").unwrap();
    assert!(!scratch.run(&["skill", "install"]).status.success());
    assert_eq!(
        fs::read_to_string(root.join("SKILL.md")).unwrap(),
        "user edits"
    );
    let custom = scratch.json(&["skill", "install", "--directory", "another agent/skills"]);
    assert!(
        scratch
            .0
            .join(custom["path"].as_str().unwrap())
            .join("SKILL.md")
            .is_file()
    );
    fs::create_dir(scratch.0.join("symlink-parent")).unwrap();
    std::os::unix::fs::symlink(&root, scratch.0.join("symlink-parent/lobo-terminal")).unwrap();
    assert!(
        !scratch
            .run(&["skill", "install", "--directory", "symlink-parent"])
            .status
            .success()
    );
}

#[test]
fn environment_defaults_are_preserved_but_not_disclosed_in_discovery() {
    let scratch = Scratch::new();
    let output = Command::new(env!("CARGO_BIN_EXE_lobo"))
        .args([
            "--preset",
            "fifo",
            "--snapshot-json",
            "--renderer",
            "cpu",
            "--capture-seconds",
            "0.1",
        ])
        .current_dir(&scratch.0)
        .env("LOBO_SYMBOL", "MSFT")
        .env_remove("LOBO_ITCH_PATH")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(state["symbol"], "MSFT");
    let output = Command::new(env!("CARGO_BIN_EXE_lobo"))
        .args(["completions", "api"])
        .env("LOBO_SYMBOL", "private-symbol")
        .env("LOBO_ITCH_PATH", "/private/data/sensitive-session")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("private-symbol"));
    assert!(!text.contains("sensitive-session"));
}
