# Completions API for agents

`lobo completions api` prints one JSON document to stdout and exits. Discovery
is offline and requires neither a TTY nor a running session. It never starts a
feed or submits an order. `lobo completion api` is an alias. This is capability
discovery, not an LLM text-completion service.

1. Check `schema_version` (currently 1) and `binary_version`. Ignore unknown fields
   within a supported schema version. Requery after upgrading the binary.
2. Select from `presets`; each provides a name, description and executable `argv`
   array. Invoke arguments directly, without shell interpolation. `--preset NAME`
   applies the same defaults. Explicit arguments and environment values win.
3. Walk `cli.subcommands` to discover nested commands. `arguments` exposes option
   spellings, aliases, help, defaults, environment names, possible values, argument
   counts and conflicts. Global options are listed once on `cli.arguments`; child
   commands mark `inherits_global_arguments`. No environment values are disclosed.
   Run the relevant command with `--help` for validation and conditional rules.
4. Read `lobo docs TOPIC --json` using an ID from `documentation`. The result is an
   array of `{id,title,content,format,resource}` entries; `lobo docs --json` lists the index and
   `lobo docs all --json` includes every guide. Unknown topics fail with a nonzero
   status. Python declarations use `python-api/MODULE` topic IDs and `format: python`.
   `lobo presets --json` returns just the preset catalog.
5. Validate a view using `lobo --preset tick-bars --bar-size 250 --renderer cpu
   --snapshot-json --capture-seconds 0.1`. Parse stdout and check exit status.
   A live feed may still be warming; never claim an empty snapshot is ready.
6. Use `interactive_commands` with `lobo ctl --attach SOCKET --execute COMMAND`
   for shared controls. Respect each entry's scope and mutation effect. Parse
   snapshot data for IDs; do not guess them. `ctl` prints acknowledgement text,
   while `--snapshot-json` prints state JSON. Errors use stderr and nonzero exit.
7. `apis` includes wire schemas and endpoints. `lobo api schema --json` returns
   that section separately; `--output PATH` writes it to disk. Discover runtime
   capabilities with the server endpoints before using hosted adapters.

Install agent guidance offline with `lobo skill install --directory .agents/skills`.
It installs `lobo-terminal/SKILL.md` and its references under that parent.
`lobo --skill` or `lobo skill show` prints the entrypoint. Existing identical
installs are accepted; differing local files and symlinks are refused.

Shell completion remains compatible: `lobo completions zsh`, `bash`, `fish`,
`elvish`, or `powershell` prints the corresponding script. For example:

```sh
lobo completions zsh > _lobo
lobo completions api > lobo-capabilities.json
lobo api schema --output lobo-api.json
```

The design follows herdr's installed help, `--skill`, `completion`/`completions`
aliases and `api schema --json/--output` workflow. Lobo additionally supplies the
explicit JSON discovery target. Sources: https://herdr.chefgroep.nl/docs/cli-reference/
and https://herdr.chefgroep.nl/docs/agent-skill/.
