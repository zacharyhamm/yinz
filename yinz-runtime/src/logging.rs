//! Yinz logging utility functions.

use std::{
    fs,
    io::{self, prelude::*},
    path::{Path, PathBuf},
    time::Instant,
};

use log::LevelFilter;

use log4rs::append::rolling_file::{
    policy::compound::{
        roll::fixed_window::FixedWindowRoller, trigger::size::SizeTrigger, CompoundPolicy,
    },
    RollingFileAppender,
};
use log4rs::config::{Appender, Config, Logger, Root};
use log4rs::encode::pattern::PatternEncoder;

use crate::consts::{YINZ_TMP_DIR, YINZ_TMP_LOG_DIR, YINZ_TMP_LOG_FILE};
use crate::shared::set_permissions;

const LOG_MAX_BYTES: u64 = 1024 * 1024 * 16; // 16 MiB per log

pub fn configure_logger() {
    atomic_create_dir(&YINZ_TMP_DIR).unwrap();
    atomic_create_dir(&YINZ_TMP_LOG_DIR).unwrap();
    atomic_create_file(&YINZ_TMP_LOG_FILE).unwrap();

    let trigger = SizeTrigger::new(LOG_MAX_BYTES);
    let roller = FixedWindowRoller::builder()
        .build(
            YINZ_TMP_LOG_DIR.join("yinz.log.old.{}").to_str().unwrap(),
            1,
        )
        .unwrap();

    // {n} means platform dependent newline
    // module is padded to exactly 25 bytes and thread is padded to be between 10 and 15 bytes.
    let file_pattern = "{highlight({level:<6})} |{module:<25.25}| {date(%Y-%m-%d %H:%M:%S.%3f)} [pid={pid}] [{thread:<10.15}] {file}:{line}: {message} {n}";

    // default yinz appender, should be used across most of the codebase.
    let log_file = RollingFileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(file_pattern)))
        .build(
            &*YINZ_TMP_LOG_FILE,
            Box::new(CompoundPolicy::new(Box::new(trigger), Box::new(roller))),
        )
        .unwrap();

    let config = Config::builder()
        .appender(Appender::builder().build("logFile", Box::new(log_file)))
        // reduce the verbosity of isahc, otherwise it logs on every failed web request
        .logger(
            Logger::builder()
                .appender("logFile")
                .build("isahc", LevelFilter::Error),
        )
        .build(Root::builder().appender("logFile").build(LevelFilter::Info))
        .unwrap();

    let _ = log4rs::init_config(config).unwrap();
}

/// Record potentially blocking startup work without logging its inputs or result.
/// An end event means the call returned, including when it returned an error.
pub fn startup_stage<T>(stage: &'static str, operation: impl FnOnce() -> T) -> T {
    log::info!("startup trace: stage={stage} event=begin");
    let started = Instant::now();
    let result = operation();
    log::info!(
        "startup trace: stage={stage} event=end elapsed_us={}",
        started.elapsed().as_micros()
    );
    result
}

pub fn atomic_create_file(file_name: &Path) -> io::Result<()> {
    let _ = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(file_name)?;
    set_permissions(file_name, 0o600)
}

pub fn atomic_create_dir(dir_name: &Path) -> io::Result<()> {
    crate::shared::ensure_secure_dir(dir_name).map_err(io::Error::other)
}

pub fn debug_to_file(message: &[u8], terminal_id: i32) -> io::Result<()> {
    let mut path = PathBuf::new();
    path.push(&*YINZ_TMP_LOG_DIR);
    path.push(format!("yinz-{}.log", terminal_id));

    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)?;
    set_permissions(&path, 0o600)?;
    file.write_all(message)
}
