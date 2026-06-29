use crate::args::{Cli, CompleteKind, ShellKind};
use crate::config::{self, Config};
use crate::git::{branch, stash, worktree};
use clap::CommandFactory;
use miette::Result;
use std::collections::HashMap;
use std::io::Write;

/// Subcommands the generated wrapper intercepts so it can `cd` into a path
/// printed on stdout (workspace navigation, plus pr open-in-workspace actions).
/// Kept in one place so all three shell templates stay in sync.
const NAV_COMMANDS: &[&str] = &["workspace", "ws", "pr", "prs", "pullrequest", "pullrequests"];

/// Generate the shell integration script (aliases + cd wrapper + completion
/// hookup) for the resolved shell, or — when `completions` is set — only the
/// static `clap_complete` completion script.
///
/// The generated script is the legitimate stdout payload (`gx setup` is meant
/// to be `eval`'d); any human-facing notices go to stderr per the project's
/// stdout-is-reserved rule.
pub fn run(
    shell: Option<ShellKind>,
    completions: Option<ShellKind>,
    name: Option<String>,
    command: Option<String>,
) -> Result<()> {
    let name = name.unwrap_or_else(|| "gx".to_string());
    let cmd = command.unwrap_or_else(|| "gx".to_string());

    // `--completions <shell>` short-circuits: emit only the static completion
    // script for that shell and return. The bin name is the wrapper `name` so
    // completion matches a custom wrapper alias (default "gx").
    if let Some(shell) = completions {
        let mut command = Cli::command();
        clap_complete::generate(
            shell.to_clap_complete(),
            &mut command,
            &name,
            &mut std::io::stdout(),
        );
        return Ok(());
    }

    let shell = shell.unwrap_or_else(detect_shell);
    let config = config::load()?;
    let config_path = config::load_path()?;

    let script = render_integration(&config, shell, &name, &cmd, &config_path.display().to_string());
    print!("{}", script);

    Ok(())
}

/// Emit dynamic completion candidates for the generated shell helpers. Invoked
/// by `main` when it intercepts `gx __complete <kind>` (deliberately not a clap
/// subcommand; see `CompleteKind`). This is the one place a completion-backing
/// path writes data to STDOUT (analogous to `print_go_path`): the shell
/// completion machinery reads it directly.
///
/// Errors (e.g. not inside a repo) print nothing and exit 0 so completion never
/// breaks the user's shell.
pub fn run_complete(kind: CompleteKind) -> Result<()> {
    let candidates = match kind {
        CompleteKind::Workspaces => collect_workspaces(),
        CompleteKind::Branches => branch::get_branches().unwrap_or_default(),
        CompleteKind::RemoteBranches => branch::get_remote_branches().unwrap_or_default(),
        CompleteKind::Stashes => collect_stashes(),
    };

    let mut out = std::io::stdout().lock();
    for candidate in candidates {
        let _ = writeln!(out, "{}", candidate);
    }

    Ok(())
}

fn collect_workspaces() -> Vec<String> {
    let Ok(worktrees) = worktree::list() else {
        return Vec::new();
    };

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for wt in worktrees {
        if seen.insert(wt.name.clone()) {
            out.push(wt.name.clone());
        }
        if let Some(branch) = wt.branch
            && seen.insert(branch.clone())
        {
            out.push(branch);
        }
    }
    out
}

fn collect_stashes() -> Vec<String> {
    stash::list()
        .map(|entries| {
            entries
                .into_iter()
                .map(|e| format!("stash@{{{}}}", e.index))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve the active shell from `$SHELL`, defaulting to zsh (the project's
/// historical POSIX/zsh behavior).
fn detect_shell() -> ShellKind {
    shell_from_path(std::env::var("SHELL").ok().as_deref())
}

/// Pure helper over an injected `$SHELL` value, for testability. Matches on the
/// path basename so `/usr/bin/zsh`, `/bin/bash`, `/opt/homebrew/bin/fish` all
/// resolve. Unknown or missing shells fall back to zsh.
fn shell_from_path(shell_env: Option<&str>) -> ShellKind {
    let Some(path) = shell_env else {
        return ShellKind::Zsh;
    };

    let basename = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match basename {
        b if b.contains("zsh") => ShellKind::Zsh,
        b if b.contains("bash") => ShellKind::Bash,
        b if b.contains("fish") => ShellKind::Fish,
        _ => ShellKind::Zsh,
    }
}

/// Build the full integration script: header, aliases, cd wrapper, and the
/// dynamic-completion hookup, for the given shell.
fn render_integration(
    config: &Config,
    shell: ShellKind,
    name: &str,
    cmd: &str,
    config_path: &str,
) -> String {
    let mut output = String::new();

    output.push_str("# GX shell integration\n");
    output.push_str(&format!("# Shell: {}\n", shell.as_str()));
    output.push_str(&format!("# Config file: {}\n", config_path));
    output.push_str("# Run: eval \"$(gx setup)\"\n");
    output.push('\n');

    output.push_str(&render_aliases(&config.aliases, cmd, shell));
    output.push('\n');

    let wrapper = match shell {
        ShellKind::Zsh => zsh_wrapper(name, cmd),
        ShellKind::Bash => bash_wrapper(name, cmd),
        ShellKind::Fish => fish_wrapper(name, cmd),
    };
    output.push_str(&wrapper);
    output.push('\n');

    output.push_str(&render_completions(shell, name, cmd));

    output
}

/// Render alias definitions sorted by alias name so output is stable
/// (config.aliases is a HashMap with run-to-run iteration order). zsh/bash use
/// `alias x='cmd ...'`; fish uses `alias x 'cmd ...'` (no `=`).
fn render_aliases(aliases: &HashMap<String, String>, cmd: &str, shell: ShellKind) -> String {
    let mut sorted: Vec<(&String, &String)> = aliases.iter().collect();
    sorted.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut out = String::from("# Aliases\n");
    for (alias, command) in sorted {
        match shell {
            ShellKind::Fish => {
                out.push_str(&format!("alias {} '{} {}'\n", alias, cmd, command));
            }
            ShellKind::Zsh | ShellKind::Bash => {
                out.push_str(&format!("alias {}='{} {}'\n", alias, cmd, command));
            }
        }
    }
    out
}

/// zsh wrapper. zsh glob-expands unquoted arguments, so branch names like
/// `feat/*` or `users/[id]` would break before reaching gx. We define a real
/// function `_<name>_run` that does the navigation logic and alias `<name>` to
/// `noglob _<name>_run`, which disables filename generation for the call.
fn zsh_wrapper(name: &str, cmd: &str) -> String {
    let func = format!("_{}_run", name);
    let mut out = String::new();
    out.push_str("# Workspace cd integration (noglob prevents zsh from expanding\n");
    out.push_str("# branch-name globs like feat/* or users/[id] before gx sees them)\n");
    out.push_str(&format!("{func}() {{\n"));
    out.push_str("    case \"$1\" in\n");
    out.push_str(&format!("        {})\n", NAV_COMMANDS.join("|")));
    out.push_str("            local __gx_out\n");
    out.push_str(&format!("            __gx_out=\"$(command {cmd} \"$@\")\" || return $?\n"));
    out.push_str("            if [ -n \"$__gx_out\" ] && [ -d \"$__gx_out\" ]; then\n");
    out.push_str("                cd \"$__gx_out\" || return 1\n");
    out.push_str("            elif [ -n \"$__gx_out\" ]; then\n");
    out.push_str("                printf '%s\\n' \"$__gx_out\"\n");
    out.push_str("            fi\n");
    out.push_str("            ;;\n");
    out.push_str("        *)\n");
    out.push_str(&format!("            command {cmd} \"$@\"\n"));
    out.push_str("            ;;\n");
    out.push_str("    esac\n");
    out.push_str("}\n");
    out.push_str(&format!("alias {name}='noglob {func}'\n"));
    out
}

/// bash wrapper. bash does not glob-expand the way zsh does for these patterns
/// (unmatched globs are passed through literally with default settings), so no
/// noglob is needed; normal quoting suffices.
fn bash_wrapper(name: &str, cmd: &str) -> String {
    let mut out = String::new();
    out.push_str("# Workspace cd integration\n");
    out.push_str(&format!("{name}() {{\n"));
    out.push_str("    case \"$1\" in\n");
    out.push_str(&format!("        {})\n", NAV_COMMANDS.join("|")));
    out.push_str("            local __gx_out\n");
    out.push_str(&format!("            __gx_out=\"$(command {cmd} \"$@\")\" || return $?\n"));
    out.push_str("            if [ -n \"$__gx_out\" ] && [ -d \"$__gx_out\" ]; then\n");
    out.push_str("                cd \"$__gx_out\" || return 1\n");
    out.push_str("            elif [ -n \"$__gx_out\" ]; then\n");
    out.push_str("                printf '%s\\n' \"$__gx_out\"\n");
    out.push_str("            fi\n");
    out.push_str("            ;;\n");
    out.push_str("        *)\n");
    out.push_str(&format!("            command {cmd} \"$@\"\n"));
    out.push_str("            ;;\n");
    out.push_str("    esac\n");
    out.push_str("}\n");
    out
}

/// fish wrapper. fish does not glob-expand unquoted args the way zsh does (an
/// unmatched glob is an error only with `set -g fish_glob...`; ordinary args are
/// passed literally), so no noglob equivalent is required.
fn fish_wrapper(name: &str, cmd: &str) -> String {
    let mut out = String::new();
    out.push_str("# Workspace cd integration\n");
    out.push_str(&format!("function {name}\n"));
    out.push_str("    switch \"$argv[1]\"\n");
    out.push_str(&format!("        case {}\n", NAV_COMMANDS.join(" ")));
    out.push_str(&format!("            set -l __gx_out (command {cmd} $argv); or return $status\n"));
    out.push_str("            if test -d \"$__gx_out\"\n");
    out.push_str("                cd \"$__gx_out\"\n");
    out.push_str("            else if test -n \"$__gx_out\"\n");
    out.push_str("                echo \"$__gx_out\"\n");
    out.push_str("            end\n");
    out.push_str("        case '*'\n");
    out.push_str(&format!("            command {cmd} $argv\n"));
    out.push_str("    end\n");
    out.push_str("end\n");
    out
}

/// Emit the static completion bootstrap plus dynamic-completion helpers that are
/// actually wired into the completion system. The static script is sourced from
/// `gx setup --completions <shell>`; the dynamic layer overrides the command's
/// completion entry point so it first runs the static (clap) completion and then
/// adds live workspace/branch/remote-branch/stash candidates for the relevant
/// argument positions.
fn render_completions(shell: ShellKind, name: &str, cmd: &str) -> String {
    match shell {
        ShellKind::Zsh => zsh_completions(name, cmd),
        ShellKind::Bash => bash_completions(name, cmd),
        ShellKind::Fish => fish_completions(name, cmd),
    }
}

fn zsh_completions(name: &str, cmd: &str) -> String {
    // The clap script defines `_<name>` and runs `compdef _<name> <name>`. We
    // wrap that function: run the static completion first, then `compadd` live
    // candidates for the relevant subcommand argument, and re-register our
    // wrapper with `compdef` so it (not clap's bare function) drives completion.
    let clap_fn = format!("_{name}");
    let dyn_fn = format!("_{name}_dynamic");
    let mut out = String::new();
    out.push_str("# Completions\n");
    out.push_str("if (( $+commands[compdef] )) || whence compdef >/dev/null 2>&1; then\n");
    out.push_str(&format!(
        "    source <(command {cmd} setup --completions zsh --name {name}) 2>/dev/null\n"
    ));
    out.push_str("    # Dynamic completion: layer live candidates over the static (clap) completion.\n");
    out.push_str(&format!("    {dyn_fn}() {{\n"));
    // Run clap's static completion (adds flags/subcommands), then augment.
    out.push_str(&format!(
        "        (( $+functions[{clap_fn}] )) && {clap_fn} \"$@\"\n"
    ));
    // $words[2] is the first subcommand, $words[3] the second (e.g. `ws go`).
    out.push_str("        case \"$words[2]\" in\n");
    out.push_str("            workspace|ws)\n");
    out.push_str("                case \"$words[3]\" in\n");
    out.push_str("                    go|switch|cd|remove|rm|delete|update|up|sync)\n");
    out.push_str(&format!(
        "                        compadd -- ${{(f)\"$(command {cmd} __complete workspaces 2>/dev/null)\"}} ;;\n"
    ));
    out.push_str("                    new|create|add)\n");
    out.push_str(&format!(
        "                        compadd -- ${{(f)\"$(command {cmd} __complete remote-branches 2>/dev/null)\"}} ;;\n"
    ));
    out.push_str("                esac ;;\n");
    out.push_str("            checkout|co|switch)\n");
    out.push_str(&format!(
        "                compadd -- ${{(f)\"$(command {cmd} __complete branches 2>/dev/null)\"}} ;;\n"
    ));
    out.push_str("            stash|st)\n");
    out.push_str(&format!(
        "                compadd -- ${{(f)\"$(command {cmd} __complete stashes 2>/dev/null)\"}} ;;\n"
    ));
    out.push_str("        esac\n");
    out.push_str("    }\n");
    out.push_str(&format!("    compdef {dyn_fn} {name}\n"));
    out.push_str("fi\n");
    out
}

fn bash_completions(name: &str, cmd: &str) -> String {
    // clap registers `complete -F _<name> ... <name>`. We wrap that function: run
    // the static completion first, then append live candidates to COMPREPLY for
    // the relevant subcommand argument, and re-register with `complete -F` so our
    // wrapper drives completion.
    let clap_fn = format!("_{name}");
    let dyn_fn = format!("_{name}_dynamic");
    let mut out = String::new();
    out.push_str("# Completions\n");
    out.push_str("if command -v complete >/dev/null 2>&1; then\n");
    out.push_str(&format!(
        "    source <(command {cmd} setup --completions bash --name {name}) 2>/dev/null\n"
    ));
    out.push_str("    # Dynamic completion: layer live candidates over the static (clap) completion.\n");
    out.push_str(&format!("    {dyn_fn}() {{\n"));
    out.push_str(&format!(
        "        declare -F {clap_fn} >/dev/null && {clap_fn} \"$@\"\n"
    ));
    out.push_str("        local cur=\"${COMP_WORDS[COMP_CWORD]}\" __gx_kind=\"\"\n");
    out.push_str("        case \"${COMP_WORDS[1]}\" in\n");
    out.push_str("            workspace|ws)\n");
    out.push_str("                case \"${COMP_WORDS[2]}\" in\n");
    out.push_str("                    go|switch|cd|remove|rm|delete|update|up|sync) __gx_kind=workspaces ;;\n");
    out.push_str("                    new|create|add) __gx_kind=remote-branches ;;\n");
    out.push_str("                esac ;;\n");
    out.push_str("            checkout|co|switch) __gx_kind=branches ;;\n");
    out.push_str("            stash|st) __gx_kind=stashes ;;\n");
    out.push_str("        esac\n");
    out.push_str("        if [ -n \"$__gx_kind\" ]; then\n");
    out.push_str(&format!(
        "            local __gx_cands; __gx_cands=\"$(command {cmd} __complete \"$__gx_kind\" 2>/dev/null)\"\n"
    ));
    out.push_str("            local __gx_word\n");
    out.push_str("            while IFS= read -r __gx_word; do\n");
    out.push_str("                COMPREPLY+=( $(compgen -W \"$__gx_word\" -- \"$cur\") )\n");
    out.push_str("            done <<< \"$__gx_cands\"\n");
    out.push_str("        fi\n");
    out.push_str("    }\n");
    out.push_str(&format!(
        "    complete -F {dyn_fn} -o bashdefault -o default {name}\n"
    ));
    out.push_str("fi\n");
    out
}

fn fish_completions(name: &str, cmd: &str) -> String {
    // fish wires dynamic candidates directly through `complete -c` rules, one per
    // kind, gated by the relevant subcommand context.
    let mut out = String::new();
    out.push_str("# Completions\n");
    out.push_str(&format!(
        "command {cmd} setup --completions fish --name {name} 2>/dev/null | source\n"
    ));
    out.push_str("# Dynamic completion helpers (workspace/branch/remote/stash refs)\n");
    // Workspace names for go/remove/update style positionals.
    out.push_str(&format!(
        "complete -c {name} -n '__fish_seen_subcommand_from workspace ws; and __fish_seen_subcommand_from go switch cd remove rm delete update up sync' -f -a '(command {cmd} __complete workspaces 2>/dev/null)'\n"
    ));
    // Remote branch names for the `workspace new <base>` positional.
    out.push_str(&format!(
        "complete -c {name} -n '__fish_seen_subcommand_from workspace ws; and __fish_seen_subcommand_from new create add' -f -a '(command {cmd} __complete remote-branches 2>/dev/null)'\n"
    ));
    // Local branch names for checkout.
    out.push_str(&format!(
        "complete -c {name} -n '__fish_seen_subcommand_from checkout co switch' -f -a '(command {cmd} __complete branches 2>/dev/null)'\n"
    ));
    // Stash refs for stash subcommands.
    out.push_str(&format!(
        "complete -c {name} -n '__fish_seen_subcommand_from stash st' -f -a '(command {cmd} __complete stashes 2>/dev/null)'\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_aliases() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("gws".to_string(), "workspace".to_string());
        m.insert("gs".to_string(), "status".to_string());
        m.insert("gco".to_string(), "checkout".to_string());
        m
    }

    #[test]
    fn test_shell_from_path_detects_known_shells() {
        assert_eq!(shell_from_path(Some("/bin/zsh")), ShellKind::Zsh);
        assert_eq!(shell_from_path(Some("/usr/bin/bash")), ShellKind::Bash);
        assert_eq!(
            shell_from_path(Some("/opt/homebrew/bin/fish")),
            ShellKind::Fish
        );
        // version-suffixed shells still resolve by substring
        assert_eq!(shell_from_path(Some("/usr/local/bin/bash-5.2")), ShellKind::Bash);
    }

    #[test]
    fn test_shell_from_path_defaults_to_zsh() {
        assert_eq!(shell_from_path(None), ShellKind::Zsh);
        assert_eq!(shell_from_path(Some("/bin/sh")), ShellKind::Zsh);
        assert_eq!(shell_from_path(Some("")), ShellKind::Zsh);
    }

    #[test]
    fn test_zsh_wrapper_includes_noglob() {
        let wrapper = zsh_wrapper("gx", "gx");
        assert!(wrapper.contains("noglob"), "zsh wrapper must contain noglob");
    }

    #[test]
    fn test_bash_wrapper_handles_navigation() {
        let wrapper = bash_wrapper("gx", "gx");
        // intercepts the navigation subcommands
        assert!(wrapper.contains("workspace|ws|pr|prs|pullrequest|pullrequests"));
        // captures stdout and cd's into a directory
        assert!(wrapper.contains("__gx_out=\"$(command gx \"$@\")\""));
        assert!(wrapper.contains("cd \"$__gx_out\""));
        // bash has no noglob
        assert!(!wrapper.contains("noglob"));
    }

    #[test]
    fn test_fish_wrapper_emits_valid_fish_syntax() {
        let wrapper = fish_wrapper("gx", "gx");
        assert!(wrapper.contains("function gx"));
        assert!(wrapper.contains("switch \"$argv[1]\""));
        assert!(wrapper.contains("case workspace ws pr prs pullrequest pullrequests"));
        assert!(wrapper.contains("set -l __gx_out (command gx $argv); or return $status"));
        assert!(wrapper.contains("case '*'"));
        // fish blocks must be balanced: every opener has a matching `end`.
        // `else if` continues a block rather than opening a new one, so only
        // count `if` openers that are not part of an `else if`.
        let if_openers = wrapper
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with("if ") || t.starts_with("if\t")
            })
            .count();
        let openers =
            wrapper.matches("function ").count() + wrapper.matches("switch ").count() + if_openers;
        let ends = wrapper.matches("end\n").count();
        assert_eq!(
            openers, ends,
            "fish function/switch/if blocks must each close with end"
        );
    }

    #[test]
    fn test_custom_command_name_is_substituted() {
        let wrapper = zsh_wrapper("gx-dev", "/Users/me/dev/gx/target/debug/gx");
        // alias uses the custom name, wrapper invokes the custom binary path
        assert!(wrapper.contains("alias gx-dev='noglob _gx-dev_run'"));
        assert!(wrapper.contains("command /Users/me/dev/gx/target/debug/gx \"$@\""));
        // the function is namespaced by the custom name
        assert!(wrapper.contains("_gx-dev_run()"));

        let bash = bash_wrapper("gx-dev", "/usr/local/bin/gx");
        assert!(bash.contains("gx-dev() {"));
        assert!(bash.contains("command /usr/local/bin/gx \"$@\""));
    }

    #[test]
    fn test_render_aliases_is_sorted_and_shell_specific() {
        let aliases = sample_aliases();

        let zsh = render_aliases(&aliases, "gx", ShellKind::Zsh);
        // sorted by alias name: gco, gs, gws
        let gco = zsh.find("alias gco").unwrap();
        let gs = zsh.find("alias gs=").unwrap();
        let gws = zsh.find("alias gws").unwrap();
        assert!(gco < gs && gs < gws, "aliases must be sorted by name");
        assert!(zsh.contains("alias gco='gx checkout'"));

        let fish = render_aliases(&aliases, "gx", ShellKind::Fish);
        // fish alias syntax has no '='
        assert!(fish.contains("alias gco 'gx checkout'"));
        assert!(!fish.contains("alias gco='gx checkout'"));
    }

    #[test]
    fn test_render_aliases_uses_custom_command() {
        let aliases = sample_aliases();
        let zsh = render_aliases(&aliases, "gx-dev", ShellKind::Zsh);
        assert!(zsh.contains("alias gco='gx-dev checkout'"));
    }

    #[test]
    fn test_completion_output_includes_workspace_subcommands() {
        // The dynamic completion hookup references the workspace subcommand and
        // the hidden __complete helper for live candidates.
        let zsh = zsh_completions("gx", "gx");
        assert!(zsh.contains("__complete workspaces"));
        assert!(zsh.contains("setup --completions zsh"));

        let fish = fish_completions("gx", "gx");
        assert!(fish.contains("__fish_seen_subcommand_from workspace ws"));
        assert!(fish.contains("__complete workspaces"));
    }

    #[test]
    fn test_zsh_completion_wires_dynamic_helper() {
        // It is not enough for the helper to be defined; it must be attached to
        // the completion system. Assert the dynamic function is registered with
        // compdef and that it both delegates to clap's static function and runs
        // compadd with live candidates.
        let zsh = zsh_completions("gx", "gx");
        // Re-registers our wrapper as the completion entry point for `gx`.
        assert!(
            zsh.contains("compdef _gx_dynamic gx"),
            "zsh must wire the dynamic helper via compdef"
        );
        // Delegates to clap's generated function so static flags still complete.
        assert!(zsh.contains("_gx \"$@\""), "zsh wrapper must call clap's _gx");
        // Actually adds live candidates for the relevant positions.
        assert!(zsh.contains("compadd"), "zsh wrapper must compadd candidates");
        // All four dynamic kinds are wired somewhere in the script.
        for kind in [
            "__complete workspaces",
            "__complete branches",
            "__complete remote-branches",
            "__complete stashes",
        ] {
            assert!(zsh.contains(kind), "zsh must wire `{kind}`");
        }
    }

    #[test]
    fn test_bash_completion_wires_dynamic_helper() {
        let bash = bash_completions("gx", "gx");
        // Re-registers our wrapper as the completion entry point for `gx`.
        assert!(
            bash.contains("complete -F _gx_dynamic"),
            "bash must wire the dynamic helper via `complete -F`"
        );
        assert!(bash.contains("_gx \"$@\""), "bash wrapper must call clap's _gx");
        assert!(
            bash.contains("COMPREPLY+="),
            "bash wrapper must append live candidates to COMPREPLY"
        );
        for kind in ["workspaces", "branches", "remote-branches", "stashes"] {
            assert!(
                bash.contains(kind),
                "bash must wire the `{kind}` completion kind"
            );
        }
    }

    #[test]
    fn test_fish_completion_wires_all_four_kinds() {
        let fish = fish_completions("gx", "gx");
        // Every dynamic completion in fish is a `complete -c gx ... -a (...)` rule.
        for kind in [
            "__complete workspaces",
            "__complete remote-branches",
            "__complete branches",
            "__complete stashes",
        ] {
            assert!(
                fish.contains("complete -c gx") && fish.contains(kind),
                "fish must wire `{kind}` via a complete rule"
            );
        }
    }

    #[test]
    fn test_dynamic_helpers_honor_custom_name() {
        let zsh = zsh_completions("gx-dev", "/usr/local/bin/gx");
        assert!(zsh.contains("compdef _gx-dev_dynamic gx-dev"));
        assert!(zsh.contains("_gx-dev \"$@\""));
        assert!(zsh.contains("command /usr/local/bin/gx __complete workspaces"));

        let bash = bash_completions("gx-dev", "/usr/local/bin/gx");
        assert!(bash.contains("complete -F _gx-dev_dynamic"));
        assert!(bash.contains("command /usr/local/bin/gx __complete"));
    }

    #[test]
    fn test_static_completion_does_not_leak_internal_complete() {
        // `__complete` is intentionally not a clap subcommand, so it must never
        // appear as a visible candidate in the generated static completion.
        for shell in [ShellKind::Zsh, ShellKind::Bash, ShellKind::Fish] {
            let mut command = Cli::command();
            let mut buf: Vec<u8> = Vec::new();
            clap_complete::generate(shell.to_clap_complete(), &mut command, "gx", &mut buf);
            let script = String::from_utf8(buf).unwrap();
            assert!(
                !script.contains("__complete"),
                "{} static completion must not expose the internal __complete helper",
                shell.as_str()
            );
        }
    }

    #[test]
    fn test_render_integration_default_zsh_order() {
        let config = Config::default();
        let script = render_integration(&config, ShellKind::Zsh, "gx", "gx", "/tmp/config.toml");
        // header, then aliases, then the wrapper, then completions
        let aliases_pos = script.find("# Aliases").unwrap();
        let wrapper_pos = script.find("Workspace cd integration").unwrap();
        let completion_pos = script.find("# Completions").unwrap();
        assert!(aliases_pos < wrapper_pos);
        assert!(wrapper_pos < completion_pos);
        assert!(script.contains("noglob"));
    }
}
