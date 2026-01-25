pub mod bundler;
pub mod cloud;
pub mod config;
pub mod har;
pub mod init;
pub mod recorder;
pub mod types;
pub mod validate;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "fusillade")]
#[command(about = "High-performance load testing engine in Rust", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run a load test scenario
    Run {
        /// Path to the scenario JS file
        scenario: PathBuf,

        /// Path to the configuration file (YAML/JSON)
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Output metrics in JSON format (newline delimited) to stdout
        #[arg(long, default_value_t = false)]
        json: bool,

        /// Export final summary to a JSON file
        #[arg(long)]
        export_json: Option<PathBuf>,

        /// Export final summary to an HTML file
        #[arg(long)]
        export_html: Option<PathBuf>,
    },
    /// Execute a script once (debug mode)
    Exec {
        /// Path to the scenario JS file
        scenario: PathBuf,
    },
    /// Start a distributed worker
    Worker {
        /// Address to listen on
        #[arg(short, long, default_value = "0.0.0.0:9876")]
        listen: String,
    },
    /// Start a distributed controller
    Controller {
        /// Path to the scenario JS file
        scenario: PathBuf,

        /// List of worker addresses (e.g., http://127.0.0.1:9876)
        #[arg(short, long)]
        workers: Vec<String>,

        /// Address to listen on for metrics
        #[arg(short, long, default_value = "0.0.0.0:9000")]
        listen: String,

        /// Path to the configuration file
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Convert a HAR file to a Fusillade JS flow
    Convert {
        /// Path to the input .har file
        #[arg(short, long)]
        input: PathBuf,

        /// Path to the output .js file
        #[arg(short, long)]
        output: PathBuf,
    },
}
