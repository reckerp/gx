use crate::config;
use miette::Result;

/// Shell wrapper that lets `gx workspace go` (and `gx pr`'s open-in-workspace /
/// troubleshoot actions) change the parent shell's directory: those commands
/// print a target path on stdout (all UI is rendered on stderr), the wrapper
/// captures it and cd's into it.
const WORKSPACE_SHELL_WRAPPER: &str = r#"# Workspace cd integration
gx() {
    case "$1" in
        workspace|ws|pr|prs|pullrequest|pullrequests)
            local __gx_out
            __gx_out="$(command gx "$@")" || return $?
            if [ -n "$__gx_out" ] && [ -d "$__gx_out" ]; then
                cd "$__gx_out" || return 1
            elif [ -n "$__gx_out" ]; then
                printf '%s\n' "$__gx_out"
            fi
            ;;
        *)
            command gx "$@"
            ;;
    esac
}
"#;

pub fn run() -> Result<()> {
    let config = config::load()?;
    let config_path = config::load_path()?;

    let mut output = String::new();

    output.push_str("# GX Aliases\n");
    output.push_str(&format!("# Config file: {}\n", config_path.display()));
    output.push_str("# Run: eval \"$(gx setup)\"\n\n");

    for (alias, command) in &config.aliases {
        output.push_str(&format!("alias {}='gx {}'\n", alias, command));
    }

    output.push('\n');
    output.push_str(WORKSPACE_SHELL_WRAPPER);

    println!("{}", output);

    Ok(())
}
