use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "ConvWatcher")]
#[command(about = "File conversion watcher daemon", long_about = None)]
pub struct Cli {
    #[arg(long, default_value_t = false)]
    pub daemon: bool,

    #[arg(long, default_value_t = false)]
    pub no_daemon: bool,

    #[arg(long, default_value = "info")]
    pub level: LogLevel,

    #[arg(short, long)]
    pub config: Option<PathBuf>,

    #[arg(short, long)]
    pub watch: Option<String>,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_log_level_filter(&self) -> log::LevelFilter {
        match self {
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Error => log::LevelFilter::Error,
        }
    }
}
