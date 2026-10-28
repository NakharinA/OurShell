mod commands;

use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = commands::Cli::parse();
    if let Err(e) = commands::run(cli).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
