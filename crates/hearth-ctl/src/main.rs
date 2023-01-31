use clap::{Parser, Subcommand};

/// Command-line interface (CLI) for interacting with a Hearth daemon over IPC.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Placeholder,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let daemon = hearth_ipc::connect()
        .await
        .expect("Failed to connect to Hearth daemon");
    println!("Hello, world!");
}
