//! frpc 下载与解压模块

use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

/// GitHub 代理地址列表，按优先级排序
const PROXY_URLS: &[&str] = &[
    "https://gitpy.223327.xyz/",
    "https://gh-proxy.com/",
    "https://github.catvod.com/",
    "https://gh.xxooo.cf/",
    "https://gh.llkk.cc/",
    "https://github.ednovas.xyz/",
    "https://gitdl.cn/",
    "https://cf.ghproxy.cc/",
    "https://ghproxy.net/",
    "https://gh.aptv.app/",
    "https://gitpr.xmcom.us.kg/",
    "https://ghproxy.cn/",
];

/// 检查 bin/ 目录下是否存在 frpc.exe
pub fn has_frpc_executable(exe_dir: &Path) -> bool {
    // 检查新的 bin/ 目录结构
    exe_dir.join("bin").join("frpc.exe").exists()
}

/// 获取最新 release 版本号（如 "v0.70.0"）
fn get_latest_release_tag(client: &reqwest::blocking::Client) -> Result<String> {
    let url = "https://api.github.com/repos/fatedier/frp/releases/latest";
    let resp = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36")
        .send()
        .context("无法获取最新版本信息")?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!(
            "获取最新版本信息失败: HTTP {}",
            resp.status()
        ));
    }

    let json: serde_json::Value = resp.json().context("解析版本信息失败")?;
    let tag = json["tag_name"]
        .as_str()
        .context("无法从 API 响应中获取 tag_name")?;

    Ok(tag.to_string())
}

/// 带进度回调的下载
fn download_with_progress(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    on_progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<()> {
    let resp = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36")
        .send()
        .context("无法发起下载请求")?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("下载失败: HTTP {}", resp.status()));
    }

    let total_size = resp.content_length().unwrap_or(0);
    let mut file = fs::File::create(dest).context("无法创建临时文件")?;
    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; 8192];
    let mut reader = resp;

    on_progress(0, total_size);

    loop {
        let bytes_read = reader.read(&mut buffer).context("读取下载数据失败")?;
        if bytes_read == 0 {
            break;
        }
        file.write_all(&buffer[..bytes_read])
            .context("写入文件失败")?;
        downloaded += bytes_read as u64;
        on_progress(downloaded, total_size);
    }

    file.flush()?;
    Ok(())
}

/// 需要从 zip 中提取的文件名
const EXTRACT_FILES: &[&str] = &["frpc.exe"];

/// 解压失败且原因是 frpc.exe 被运行中的进程占用（Windows 下无法替换运行中的 exe）
#[derive(Debug)]
struct FileInUseError;

impl std::fmt::Display for FileInUseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "frpc.exe 正在运行，文件被占用")
    }
}

impl std::error::Error for FileInUseError {}

/// 判断错误是否为 Windows 下“文件被进程占用/锁定”
fn is_file_in_use_err(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        // ERROR_ACCESS_DENIED=5, ERROR_SHARING_VIOLATION=32, ERROR_LOCK_VIOLATION=33
        matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33))
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

/// 从 zip 文件中提取 frpc.exe 到目标目录（bin/）
fn extract_frpc_from_zip(zip_path: &Path, dest_dir: &Path) -> Result<()> {
    // 确保 bin/ 目录存在
    fs::create_dir_all(dest_dir).context("无法创建 bin 目录")?;

    let file = fs::File::open(zip_path).context("无法打开下载的 zip 文件")?;
    let mut archive = zip::ZipArchive::new(file).context("无法解析 zip 文件")?;

    let mut found_exe = false;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("无法读取 zip 条目")?;
        let entry_name = entry.mangled_name();

        let file_name = match entry_name.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => continue,
        };

        // 只提取 frpc.exe（可能在子目录中，如 frp_0.70.0_windows_amd64/）
        if EXTRACT_FILES.contains(&file_name.as_str()) {
            let out_path = dest_dir.join(&*file_name);
            let tmp_path = dest_dir.join(format!("{}.tmp", file_name));

            // 先解压到临时文件，避免中途失败破坏现有的 frpc.exe
            let mut out_file = match fs::File::create(&tmp_path) {
                Ok(f) => f,
                Err(e) => return Err(anyhow::anyhow!("无法创建 {}: {}", file_name, e)),
            };
            if let Err(e) = std::io::copy(&mut entry, &mut out_file) {
                let _ = fs::remove_file(&tmp_path);
                return Err(anyhow::anyhow!("解压 {} 失败: {}", file_name, e));
            }
            drop(out_file);

            // 用临时文件替换目标文件。Windows 下若 frpc.exe 正在运行，重命名会失败；
            // 这里重试几次以容忍杀毒软件等造成的短暂文件占用。
            let mut last_err: Option<std::io::Error> = None;
            for _ in 0..3 {
                match fs::rename(&tmp_path, &out_path) {
                    Ok(()) => {
                        last_err = None;
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        std::thread::sleep(std::time::Duration::from_millis(300));
                    }
                }
            }
            let _ = fs::remove_file(&tmp_path);

            if let Some(e) = last_err {
                if is_file_in_use_err(&e) {
                    // frpc.exe 正被运行中的进程锁定，无法替换
                    return Err(FileInUseError.into());
                }
                return Err(anyhow::anyhow!("无法替换 {}: {}", file_name, e));
            }

            log::info!("已将 {} 解压到 {:?}", file_name, out_path);
            if file_name == "frpc.exe" {
                found_exe = true;
            }
        }
    }

    if found_exe {
        Ok(())
    } else {
        Err(anyhow::anyhow!("下载的 zip 文件中未找到 frpc.exe"))
    }
}

/// 检查是否有新版本可用
///
/// 返回 Some(latest_tag) 如果有更新，None 如果已是最新
pub fn check_update() -> Result<Option<String>> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let bin_dir = exe_dir.join("bin");
    let exe_path = bin_dir.join("frpc.exe");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("创建 HTTP 客户端失败")?;

    let tag = get_latest_release_tag(&client)?;

    if exe_path.exists() {
        let mut cmd = std::process::Command::new(&exe_path);
        cmd.arg("--version");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        if let Ok(output) = cmd.output() {
            let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if version_str.contains(tag.trim_start_matches('v')) {
                return Ok(None);
            }
        }
    }

    Ok(Some(tag))
}

/// 主入口：下载并解压 frpc.exe 到 bin/ 目录
///
/// `program_dir` 为程序所在目录，frpc.exe 会解压到 `program_dir/bin/` 下。
/// 进度通过 `on_progress(downloaded_bytes, total_bytes)` 回调报告
pub fn download_and_extract_frpc(
    program_dir: &Path,
    on_progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<()> {
    // 下载目标为 bin/ 子目录
    let bin_dir = program_dir.join("bin");
    // 确保 bin/ 目录存在（下载临时文件需要写入此目录）
    fs::create_dir_all(&bin_dir).context("无法创建 bin 目录")?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("创建 HTTP 客户端失败")?;

    // 1. 获取最新版本号
    let tag = get_latest_release_tag(&client)?;
    log::info!("获取到最新 frp 版本: {}", tag);

    // 检查是否已是最新版本，跳过下载
    let exe_path = bin_dir.join("frpc.exe");
    if exe_path.exists() {
        let mut cmd = std::process::Command::new(&exe_path);
        cmd.arg("--version");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        if let Ok(output) = cmd.output() {
            let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if version_str.contains(tag.trim_start_matches('v')) {
                log::info!("frpc 已是最新版本 ({}), 跳过下载", tag);
                return Ok(());
            }
        }
    }

    let file_name = format!("frp_{}_windows_amd64.zip", tag.trim_start_matches('v'));
    let github_url = format!(
        "https://github.com/fatedier/frp/releases/download/{}/{}",
        tag, file_name
    );

    // 2. 构建候选 URL 列表（原始地址 + 代理地址）
    let mut candidate_urls = Vec::new();
    candidate_urls.push(github_url.clone());
    for proxy in PROXY_URLS {
        candidate_urls.push(format!("{}{}", proxy, github_url));
    }

    // 3. 依次尝试下载
    let zip_path = bin_dir.join("__frpc_download_temp.zip");
    let mut last_error = String::new();

    for url in &candidate_urls {
        log::info!("尝试下载: {}", url);
        match download_with_progress(&client, url, &zip_path, on_progress) {
            Ok(()) => {
                log::info!("下载成功: {}", url);
                // 解压到 bin/ 目录
                match extract_frpc_from_zip(&zip_path, &bin_dir) {
                    Ok(()) => {
                        let _ = fs::remove_file(&zip_path);
                        return Ok(());
                    }
                    Err(e) => {
                        let _ = fs::remove_file(&zip_path);
                        // frpc.exe 被运行中的进程占用：换镜像重试毫无意义（每次都会解压失败），直接返回
                        if e.downcast_ref::<FileInUseError>().is_some() {
                            return Err(anyhow::anyhow!(
                                "frpc.exe 正在运行或已被占用，无法替换。请先停止所有 frpc 进程后重试。"
                            ));
                        }
                        last_error = format!("解压失败: {}", e);
                        log::warn!("解压失败 ({}): {}", url, e);
                        continue;
                    }
                }
            }
            Err(e) => {
                last_error = format!("下载失败: {}", e);
                log::warn!("下载失败 ({}): {}", url, e);
                let _ = fs::remove_file(&zip_path);
                continue;
            }
        }
    }

    Err(anyhow::anyhow!(
        "所有下载地址均失败，最后的错误: {}",
        last_error
    ))
}
