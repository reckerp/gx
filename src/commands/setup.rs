use crate::config;
use miette::Result;

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

    println!("{}", output);

    Ok(())
}
