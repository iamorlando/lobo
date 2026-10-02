//! Offline discovery is safe to call without a TTY, GPU, feed, or server.
use crate::{
    args::{ApiAction, Args, CompletionTarget, SkillAction, View},
    presets::PRESETS,
};
use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use serde_json::{Value, json};
use std::{fs, io, path::Path};

include!(concat!(env!("OUT_DIR"), "/skill_bundle.rs"));

pub fn skill() -> &'static str {
    SKILL_FILES
        .iter()
        .find(|(path, _)| *path == "SKILL.md")
        .unwrap()
        .1
}

pub fn documents() -> Vec<Value> {
    SKILL_FILES
        .iter()
        .filter_map(|(path, content)| {
            let (id, format) = if let Some(id) = path.strip_prefix("references/docs/").and_then(|p| p.strip_suffix(".md")) {
                (id.to_owned(), "markdown")
            } else if let Some(id) = path.strip_prefix("references/python-api/").and_then(|p| p.strip_suffix(".pyi")) {
                (format!("python-api/{id}"), "python")
            } else {
                return None;
            };
            let title = content
                .lines()
                .find_map(|l| l.strip_prefix("# "))
                .unwrap_or(&id);
            Some(json!({"id": id, "title": title, "content": content, "format": format, "resource": path}))
        })
        .collect()
}

fn command_metadata(command: &clap::Command, root: bool) -> Value {
    // Global options are listed once on the root, not repeated for every leaf.
    let arguments: Vec<Value> = command.get_arguments().filter(|a| !a.is_hide_set() && (root || !a.is_global_set())).map(|arg| json!({
        "id": arg.get_id().as_str(), "long": arg.get_long(), "short": arg.get_short(),
        "aliases": arg.get_all_aliases().unwrap_or_default(),
        "help": arg.get_long_help().or(arg.get_help()).map(ToString::to_string),
        "required": arg.is_required_set(), "global": arg.is_global_set(),
        "action": format!("{:?}", arg.get_action()),
        "num_args": arg.get_num_args().map(|n| json!({"min": n.min_values(), "max": n.max_values()})),
        "value_names": arg.get_value_names().map(|v| v.iter().map(ToString::to_string).collect::<Vec<_>>()),
        "possible_values": arg.get_possible_values().iter().filter(|v| !v.is_hide_set()).map(|v| v.get_name()).collect::<Vec<_>>(),
        "defaults": arg.get_default_values().iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>(),
        "environment": arg.get_env().map(|s| s.to_string_lossy()),
        "conflicts": command.get_arg_conflicts_with(arg).iter().map(|a| a.get_id().as_str()).collect::<Vec<_>>()
    })).collect();
    json!({
        "name": command.get_name(), "aliases": command.get_all_aliases().collect::<Vec<_>>(),
        "description": command.get_long_about().or(command.get_about()).map(ToString::to_string),
        "usage": command.clone().render_usage().to_string(),
        "inherits_global_arguments": !root,
        "after_help": command.get_after_long_help().or(command.get_after_help()).map(ToString::to_string),
        "arguments": arguments,
        "subcommands": command.get_subcommands().filter(|c| !c.is_hide_set()).map(|c| command_metadata(c, false)).collect::<Vec<_>>()
    })
}

pub fn schemas() -> Value {
    use lobo_models::server::{
        BookInfo, Command, CommandResponse, FeedMessage, IcebergOrder, LimitOrder, MarketOrder,
    };
    json!({
        "schema_version": 1,
        "binary_version": env!("CARGO_PKG_VERSION"),
        "snapshot": schemars::schema_for!(crate::engine::Snapshot),
        "session": {"protocol": crate::shared::PROTOCOL, "framing": "4-byte big-endian length followed by UTF-8 JSON; maximum 64 MiB", "schemas": crate::shared::schemas()},
        "http": {
            "command": schemars::schema_for!(Command),
            "response": schemars::schema_for!(CommandResponse),
            "feed": schemars::schema_for!(FeedMessage),
            "book": schemars::schema_for!(BookInfo),
            "market": schemars::schema_for!(MarketOrder),
            "limit": schemars::schema_for!(LimitOrder),
            "iceberg": schemars::schema_for!(IcebergOrder),
            "runtime_schema": "/api/schema",
            "note": "Read /api/server-context and /api/schema on the target host for adapter descriptors, adapter envelopes and enabled capabilities. HTTP prices/quantities are integer atoms; terminal inputs are displayed units."
        },
        "endpoints": serde_json::from_str::<Value>(include_str!("../docs/agents/endpoints.json")).unwrap()
    })
}

pub fn catalog() -> Value {
    let mut command = Args::command();
    command.build();
    json!({
        "schema_version": 1,
        "binary": "lobo", "binary_version": env!("CARGO_PKG_VERSION"),
        "agent_usage": {
            "start": "Read this catalog, select a preset, and inspect relevant docs with lobo docs TOPIC --json.",
            "launch": "Run lobo --preset NAME with explicit overrides. For headless validation add --renderer cpu --snapshot-json --capture-seconds 0.1.",
            "control": "Start lobo session --socket PATH, attach views with --attach PATH, then use lobo ctl --attach PATH --execute COMMAND.",
            "safety": "Discovery does not execute commands. Do not probe bare lobo: it starts a TUI. Order mutations require the user's intent; never retry a write after an uncertain acknowledgement.",
            "compatibility": "Check schema_version before consuming the catalog; ignore unknown fields. This is a discovery API, not an LLM text-generation endpoint.",
            "skill": "lobo skill install --directory .agents/skills"
        },
        "cli": command_metadata(&command, true),
        "interactive_commands": serde_json::from_str::<Value>(include_str!("../docs/agents/commands.json")).unwrap(),
        "presets": PRESETS,
        "documentation": documents().into_iter().map(|mut d| { d.as_object_mut().unwrap().remove("content"); d }).collect::<Vec<_>>(),
        "apis": schemas()
    })
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Install only into a new leaf, or verify an identical prior install. Never
/// overwrite a locally edited skill or follow an existing leaf symlink.
pub fn install(directory: &Path) -> Result<std::path::PathBuf> {
    fs::create_dir_all(directory).with_context(|| format!("create {}", directory.display()))?;
    let destination = directory.join("lobo-terminal");
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!(
                "refusing to replace {}: expected a real skill directory",
                destination.display()
            );
        }
        for (name, content) in SKILL_FILES {
            let file = destination.join(name);
            // Reject symlinks in every relative component, not just the leaf.
            let mut component = destination.clone();
            for part in Path::new(name).components() {
                component.push(part);
                if fs::symlink_metadata(&component).is_ok_and(|m| m.file_type().is_symlink()) {
                    bail!("refusing to follow skill symlink {}", component.display());
                }
            }
            if fs::read(&file).ok().as_deref() != Some(content.as_bytes()) {
                bail!(
                    "{} differs from this binary's skill; choose another --directory or back up and remove the old skill first",
                    file.display()
                );
            }
        }
        return Ok(destination);
    }
    // Staging and create_dir avoid merging with a concurrent or partial install.
    let staging = directory.join(format!(".lobo-terminal-{}", std::process::id()));
    fs::create_dir(&staging).context("create skill staging directory")?;
    let result = (|| {
        for (name, content) in SKILL_FILES {
            let file = staging.join(name);
            fs::create_dir_all(file.parent().unwrap())?;
            fs::write(file, content)?;
        }
        if destination.exists() {
            bail!("skill destination appeared during installation; retry");
        }
        fs::rename(&staging, &destination)?;
        Ok(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

pub fn run(args: &Args) -> Result<bool> {
    if args.skill {
        print!("{}", skill());
        return Ok(true);
    }
    match &args.command {
        Some(View::Completions { target }) => {
            let shell = match target {
                CompletionTarget::Api => {
                    print_json(&catalog())?;
                    return Ok(true);
                }
                CompletionTarget::Bash => clap_complete::Shell::Bash,
                CompletionTarget::Elvish => clap_complete::Shell::Elvish,
                CompletionTarget::Fish => clap_complete::Shell::Fish,
                CompletionTarget::Powershell => clap_complete::Shell::PowerShell,
                CompletionTarget::Zsh => clap_complete::Shell::Zsh,
            };
            clap_complete::generate(shell, &mut Args::command(), "lobo", &mut io::stdout());
        }
        Some(View::Docs { topic, json }) => {
            let docs = documents();
            if let Some(topic) = topic {
                let found: Vec<_> = docs
                    .into_iter()
                    .filter(|d| topic == "all" || d["id"] == *topic)
                    .collect();
                if found.is_empty() {
                    bail!("unknown documentation topic {topic:?}; run `lobo docs` for the index");
                }
                if *json {
                    print_json(&found)?;
                } else {
                    for doc in found {
                        println!("{}", doc["content"].as_str().unwrap());
                    }
                }
            } else {
                let index: Vec<_> = docs
                    .into_iter()
                    .map(|mut d| {
                        d.as_object_mut().unwrap().remove("content");
                        d
                    })
                    .collect();
                if *json {
                    print_json(&index)?;
                } else {
                    for doc in index {
                        println!(
                            "{:<24} {}",
                            doc["id"].as_str().unwrap(),
                            doc["title"].as_str().unwrap()
                        );
                    }
                }
            }
        }
        Some(View::Presets { json }) => {
            if *json {
                print_json(&PRESETS)?;
            } else {
                for preset in PRESETS {
                    println!(
                        "{:<15} {}\n  lobo {}",
                        serde_json::to_value(preset.name)?.as_str().unwrap(),
                        preset.description,
                        preset.argv.join(" ")
                    );
                }
            }
        }
        Some(View::Api {
            action: ApiAction::Schema { json, output },
        }) => {
            let schema = schemas();
            if let Some(path) = output {
                fs::write(
                    path,
                    format!("{}\n", serde_json::to_string_pretty(&schema)?),
                )
                .with_context(|| format!("write {}", path.display()))?;
            }
            if *json {
                print_json(&schema)?;
            } else if output.is_none() {
                println!(
                    "Lobo API schema v1 · binary {}\nSnapshot, session protocol {}, HTTP order/feed schemas and endpoint catalog.\nUse `lobo api schema --json` or `--output PATH` for the full document.\nAgents: `lobo completions api` discovers commands; `lobo docs api` explains use.",
                    env!("CARGO_PKG_VERSION"),
                    crate::shared::PROTOCOL
                );
            }
        }
        Some(View::Skill { action }) => match action {
            None | Some(SkillAction::Show) => print!("{}", skill()),
            Some(SkillAction::Install { directory }) => {
                let path = install(directory)?;
                print_json(
                    &json!({"skill": "lobo-terminal", "path": path, "binary_version": env!("CARGO_PKG_VERSION")}),
                )?;
            }
        },
        _ => return Ok(false),
    }
    Ok(true)
}
