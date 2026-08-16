//! 日志配置与清理，按天存储日志并自动清理超过 30 天的日志文件

use anyhow::{Context, Result};
use chrono::Local;
use log::LevelFilter;
use log4rs::{
    append::Append,
    config::{Appender, Config, Root},
};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;

/// 自适应文件写入器：每次写入时以 append + create 模式打开文件，
/// 文件被外部删除后下次写入自动重建，无需定期检查。
struct ResilientWriter {
    path: PathBuf,
    file: Mutex<Option<fs::File>>,
}

impl std::fmt::Debug for ResilientWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResilientWriter")
            .field("path", &self.path)
            .finish()
    }
}

impl ResilientWriter {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: Mutex::new(None),
        }
    }

    fn write_log(&self, record: &log::Record) {
        let mut guard = self.file.lock().unwrap();

        // 如果没有文件句柄，尝试打开（不存在则创建）
        if guard.is_none() {
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(f) => *guard = Some(f),
                Err(e) => {
                    eprintln!("无法打开日志文件 {:?}: {}", self.path, e);
                    return;
                }
            }
        }

        if let Some(ref mut file) = *guard {
            let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
            let level = record.level();
            let args = record.args();
            let line = format!("{} [{}] {}\n", timestamp, level, args);
            if file.write_all(line.as_bytes()).is_err() {
                // 写入失败（文件可能被删除），丢弃句柄，下次重建
                *guard = None;
            }
        }
    }
}

impl Append for ResilientWriter {
    fn append(&self, record: &log::Record) -> anyhow::Result<()> {
        self.write_log(record);
        Ok(())
    }

    fn flush(&self) {
        let guard = self.file.lock().unwrap();
        if let Some(ref file) = *guard {
            let _ = file.sync_all();
        }
    }
}

/// 当前日志级别，供轮转线程和运行时切换使用
static LOG_LEVEL: Mutex<LevelFilter> = Mutex::new(LevelFilter::Info);
/// 日志系统句柄，用于运行时动态调整日志级别
static LOG_HANDLE: OnceLock<log4rs::Handle> = OnceLock::new();

/// 初始化日志系统，并启动后台线程在每天零点自动切换日志文件
pub fn init_logging() -> Result<()> {
    let exe_path = env::current_exe().context("无法获取可执行文件路径")?;
    let exe_dir = exe_path.parent().context("无法获取可执行文件目录")?;

    let logs_dir = exe_dir.join("logs");
    fs::create_dir_all(&logs_dir).context("无法创建日志目录")?;

    // 从设置中读取日志级别（未设置或非法时回退到 Info）
    let level = crate::config::load_settings()
        .log_level
        .parse()
        .unwrap_or(LevelFilter::Info);

    // 构建今天的日志配置
    let config = build_log_config(&logs_dir, level)?;

    let handle = log4rs::init_config(config).context("无法初始化日志")?;
    *LOG_LEVEL.lock().unwrap() = level;
    let _ = LOG_HANDLE.set(handle.clone());

    // 确认日志文件已创建并写入首条记录
    log::info!("日志系统初始化完成，日志目录: {:?}", logs_dir);

    // 首次启动时清理超过 30 天的旧日志
    let _ = clean_old_logs(&logs_dir);

    // 启动后台线程：在每天零点切换到新的日志文件并清理过期日志
    let handle_clone = handle.clone();
    thread::spawn(move || {
        log_rotation_loop(handle_clone, &logs_dir);
    });

    Ok(())
}

/// 构建指向当天日志文件的 Config
fn build_log_config(logs_dir: &Path, level: LevelFilter) -> Result<Config> {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let log_file = logs_dir.join(format!("{}.log", today));

    let writer = ResilientWriter::new(log_file);

    Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(writer)))
        .build(Root::builder().appender("logfile").build(level))
        .context("无法构建日志配置")
}

/// 运行时切换日志级别
pub fn set_log_level(level: LevelFilter) -> Result<()> {
    let exe_path = env::current_exe().context("无法获取可执行文件路径")?;
    let exe_dir = exe_path.parent().context("无法获取可执行文件目录")?;
    let logs_dir = exe_dir.join("logs");

    let config = build_log_config(&logs_dir, level)?;
    if let Some(handle) = LOG_HANDLE.get() {
        handle.set_config(config);
    }
    *LOG_LEVEL.lock().unwrap() = level;
    log::info!("日志级别已切换为: {}", level);
    Ok(())
}

/// 后台日志轮转循环：每天零点切换到新的日志文件并清理过期日志
fn log_rotation_loop(handle: log4rs::Handle, logs_dir: &Path) {
    let mut last_date = Local::now().format("%Y-%m-%d").to_string();

    loop {
        // 计算距离下一个零点需要等待的秒数
        let wait_secs = {
            let now = Local::now();
            let tomorrow = (now + chrono::Duration::days(1))
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap();
            (tomorrow - now.naive_local()).num_seconds().max(1) as u64
        };

        thread::sleep(std::time::Duration::from_secs(wait_secs));

        // 切换到新日期的日志文件
        let today = Local::now().format("%Y-%m-%d").to_string();
        if today != last_date {
            let level = *LOG_LEVEL.lock().unwrap();
            match build_log_config(logs_dir, level) {
                Ok(new_config) => {
                    handle.set_config(new_config);
                    log::info!("日志文件已切换到 {}", today);
                    let _ = clean_old_logs(logs_dir);
                    last_date = today;
                }
                Err(e) => eprintln!("日志轮转失败: {:?}", e),
            }
        }
    }
}

/// 清理超过 30 天的日志文件（按文件名中的日期判断，格式 YYYY-MM-DD.log）
fn clean_old_logs(logs_dir: &Path) -> Result<()> {
    let cutoff = (Local::now() - chrono::Duration::days(30)).date_naive();

    let entries = fs::read_dir(logs_dir).context("无法列出日志目录")?;

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        // 只处理 YYYY-MM-DD.log 格式的文件
        let date_str = match name.strip_suffix(".log") {
            Some(s) => s,
            None => continue,
        };

        let file_date = match chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => continue,
        };

        if file_date < cutoff {
            if let Err(e) = fs::remove_file(entry.path()) {
                eprintln!("删除旧日志 {:?} 失败: {}", entry.path(), e);
            }
        }
    }

    Ok(())
}
