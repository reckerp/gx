use miette::Result;

pub fn run(query: Option<String>) -> Result<()> {
    if let Some(query) = query {
        println!("{}", query);
    }
    Ok(())
}
