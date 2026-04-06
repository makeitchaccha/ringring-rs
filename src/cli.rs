use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ringring-rs")]
pub struct Cli {
    #[arg(long, default_value = "./config.toml", help = "Config file")]
    pub config: PathBuf,
}
