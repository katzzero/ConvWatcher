pub mod error_logger;

use log::LevelFilter;

/// Setup logging for ConvWatcher.
pub fn setup_logging(
    cli_level: &str,
    app_log_file: Option<&str>,
    daemon: bool,
    no_daemon: bool,
) -> anyhow::Result<()> {
    let log_dir = "logs";

    // Use configured path or default to logs directory
    let log_file_path = app_log_file.unwrap_or("./logs/convwatcher.log");

    let _ = std::fs::create_dir_all(log_dir);

    let level: LevelFilter = cli_level.parse().unwrap_or(LevelFilter::Info);

    let mut dispatch = fern::Dispatch::new()
        .level(level)
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                message
            ))
        });

    // `--daemon` only redirects logs to a file (it does NOT fork/detach the
    // process). `--no_daemon` forces console logging even if `--daemon` is set.
    if daemon && !no_daemon {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file_path)
        {
            Ok(log_file) => dispatch = dispatch.chain(log_file),
            Err(e) => eprintln!("Warning: cannot open log file {}: {}", log_file_path, e),
        }
    } else {
        dispatch = dispatch.chain(
            fern::Dispatch::new()
                .level(level)
                .filter(|meta| meta.level() <= log::Level::Warn)
                .chain(std::io::stderr()),
        );
        dispatch = dispatch.chain(fern::Dispatch::new().level(level).chain(std::io::stdout()));
        if let Ok(log_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file_path)
        {
            dispatch = dispatch.chain(fern::Dispatch::new().level(level).chain(log_file));
        }
    }

    dispatch.apply()?;
    Ok(())
}
