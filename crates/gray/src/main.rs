use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "gray", version, about = "Minimal modular agent harness")]
pub struct Cli {
    /// Model to use (e.g. provider/model-id)
    #[arg(long)]
    pub model: Option<String>,

    /// Custom API base URL
    #[arg(long)]
    pub base_url: Option<String>,

    /// Print mode: execute prompt directly and print output
    #[arg(short = 'p', long = "print")]
    pub print: Option<String>,
}

fn main() {
    let _cli = Cli::parse();
    println!("gray");
}
