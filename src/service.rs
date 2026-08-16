//! Windows 服务管理：注册/注销/状态检查 + 服务调度器
//!

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::{
    InitializeSecurityDescriptor, SetSecurityDescriptorDacl, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE,
    OPEN_EXISTING, PIPE_ACCESS_INBOUND,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

/// 服务停止信号，由 SCM 停止事件设置
static SERVICE_STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

// Event access constants
const EVENT_MODIFY_STATE: u32 = 0x0002;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x102;
const GENERIC_WRITE: u32 = 0x40000000;

/// Named pipe for guard_stopped IPC: UI sends STOP/START/CLEAR commands
const PIPE_NAME: &str = "\\\\.\\pipe\\FrpcGuardStopped";

fn pipe_name_utf16() -> Vec<u16> {
    PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect()
}

/// "Global\FrpcGuardChanged" as UTF-16 with null terminator
fn guard_event_name() -> Vec<u16> {
    "Global\\FrpcGuardChanged\0".encode_utf16().collect()
}

/// Create a named event with NULL DACL (allows cross-session access)
///
/// The service runs as SYSTEM (session 0) and the UI runs as the user (session 1+).
/// A NULL DACL grants everyone access to the named event.
fn create_named_event(name: &[u16], desc: &str) -> Result<HANDLE> {
    let mut sd: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };

    unsafe {
        if InitializeSecurityDescriptor(&mut sd as *mut _ as *mut _, 1) == 0 {
            return Err(anyhow::anyhow!("无法初始化安全描述符"));
        }
        // NULL DACL = everyone has access
        if SetSecurityDescriptorDacl(&mut sd as *mut _ as *mut _, 1, std::ptr::null(), 0) == 0 {
            return Err(anyhow::anyhow!("无法设置安全描述符 DACL"));
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: &mut sd as *mut _ as *mut _,
            bInheritHandle: 0,
        };
        let event = CreateEventW(&sa, 0, 0, name.as_ptr());
        if event == 0 {
            return Err(anyhow::anyhow!("无法创建{}事件", desc));
        }
        Ok(event)
    }
}

/// Signal a named event by name. Does nothing if the event doesn't exist.
fn signal_named_event(name: &[u16]) {
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if event != 0 {
            SetEvent(event);
            CloseHandle(event);
        }
    }
}

/// Signal the guard change event to wake up the service.
/// Called by the UI after toggling process guard.
pub fn signal_guard_changed() {
    signal_named_event(&guard_event_name());
}

/// "Global\FrpcProcessChanged" as UTF-16 with null terminator
fn process_changed_event_name() -> Vec<u16> {
    "Global\\FrpcProcessChanged\0".encode_utf16().collect()
}

/// Signal the process changed event to wake up the UI.
/// Called by the service after restarting a process via guard.
pub fn signal_process_changed() {
    signal_named_event(&process_changed_event_name());
}

/// Wait for the process changed event or timeout.
/// Returns true if the event was signaled, false if timed out.
/// Used by the UI health monitor to react immediately to service restarts.
pub fn wait_process_changed(timeout_ms: u32) -> bool {
    const SYNCHRONIZE: u32 = 0x00100000;
    unsafe {
        let event = OpenEventW(SYNCHRONIZE, 0, process_changed_event_name().as_ptr());
        if event == 0 {
            std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            return false;
        }
        let result = WaitForSingleObject(event, timeout_ms);
        CloseHandle(event);
        result == WAIT_OBJECT_0
    }
}

/// UI 调用：通过命名管道向 Service 发送命令
///
/// 命令格式：
/// - `STOP:config_name` — 将配置加入手动停止列表
/// - `START:config_name` — 将配置从手动停止列表移除
/// - `CLEAR` — 清空手动停止列表
/// - `TRACK:config_name:pid` — 通知 Service 将 UI 启动的进程纳入守护跟踪
pub fn send_guard_stopped_command(command: &str) {
    // 重试 3 次，每次间隔 50ms，应对管道短暂不可用的情况
    // （DisconnectNamedPipe 到下一次 CreateNamedPipeW 之间的间隙）
    let mut last_error: u32 = 0;
    for attempt in 0..3u32 {
        unsafe {
            let handle = CreateFileW(
                pipe_name_utf16().as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                0,
            );
            if handle == INVALID_HANDLE_VALUE {
                last_error = GetLastError();
                if attempt < 2 {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                break;
            }
            let data = format!("{}\n", command);
            let mut bytes_written = 0u32;
            WriteFile(
                handle,
                data.as_ptr(),
                data.len() as u32,
                &mut bytes_written,
                std::ptr::null_mut(),
            );
            FlushFileBuffers(handle);
            CloseHandle(handle);
            return;
        }
    }

    // 管道不存在（ERROR_FILE_NOT_FOUND=2）通常表示服务未运行，
    // 此时 STOP/START 等命令没有接收方，属正常情况，只记调试日志；
    // 其他错误（拒绝访问、持续占用等）才需要告警提示。
    if last_error == 2 {
        log::debug!("服务未运行，跳过管道命令 '{}' ({})", command, PIPE_NAME);
    } else {
        log::warn!(
            "无法连接到命名管道 {}（已重试 3 次，错误码 {}）: {}",
            PIPE_NAME,
            last_error,
            command
        );
    }
}

/// 创建命名管道服务器（带 NULL DACL，允许跨会话访问）
fn create_named_pipe_server() -> Result<HANDLE> {
    let mut sd: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
    unsafe {
        if InitializeSecurityDescriptor(&mut sd as *mut _ as *mut _, 1) == 0 {
            return Err(anyhow::anyhow!("无法初始化安全描述符"));
        }
        if SetSecurityDescriptorDacl(&mut sd as *mut _ as *mut _, 1, std::ptr::null(), 0) == 0 {
            return Err(anyhow::anyhow!("无法设置安全描述符 DACL"));
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: &mut sd as *mut _ as *mut _,
            bInheritHandle: 0,
        };
        let handle = CreateNamedPipeW(
            pipe_name_utf16().as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            0,
            4096,
            0,
            &sa,
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(anyhow::anyhow!("无法创建命名管道"));
        }
        Ok(handle)
    }
}

/// 启动命名管道监听线程，接收 UI 发送的命令（STOP/START/CLEAR/TRACK）
fn start_guard_stopped_pipe(
    guard_stopped: Arc<Mutex<HashSet<String>>>,
    processes: Arc<Mutex<Vec<(String, FrpcProcess)>>>,
    auto_start_map: Arc<std::collections::HashMap<String, (PathBuf, PathBuf)>>,
) {
    thread::spawn(move || {
        loop {
            let pipe = match create_named_pipe_server() {
                Ok(h) => h,
                Err(e) => {
                    log::error!("创建命名管道失败: {:?}", e);
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
            };

            // 等待客户端连接
            if unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } == 0 {
                let err = unsafe { GetLastError() };
                // ERROR_PIPE_CONNECTED (535) 也算连接成功
                if err != 535 {
                    unsafe { CloseHandle(pipe) };
                    continue;
                }
            }

            // 读取数据
            let mut buffer = [0u8; 4096];
            let mut bytes_read = 0u32;
            let success = unsafe {
                ReadFile(
                    pipe,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut bytes_read,
                    std::ptr::null_mut(),
                )
            };

            if success != 0 && bytes_read > 0 {
                let data = String::from_utf8_lossy(&buffer[..bytes_read as usize]);
                for line in data.lines() {
                    let line = line.trim();
                    if let Some(name) = line.strip_prefix("STOP:") {
                        let mut gs = guard_stopped.lock().unwrap();
                        gs.insert(name.to_string());
                        log::info!("[{}] 已加入手动停止列表（管道）", name);
                    } else if let Some(name) = line.strip_prefix("START:") {
                        let mut gs = guard_stopped.lock().unwrap();
                        gs.remove(name);
                        log::info!("[{}] 已从手动停止列表移除（管道）", name);
                    } else if line == "CLEAR" {
                        let mut gs = guard_stopped.lock().unwrap();
                        gs.clear();
                        log::info!("手动停止列表已清空（管道）");
                    } else if let Some(remainder) = line.strip_prefix("TRACK:") {
                        // UI 启动了进程，通知 Service 纳入守护跟踪
                        // 格式: TRACK:config_name:pid
                        if let Some((name, pid_str)) = remainder.split_once(':') {
                            if let Ok(pid) = pid_str.parse::<u32>() {
                                if let Some((exe, conf)) = auto_start_map.get(name) {
                                    let mut proc_list = processes.lock().unwrap();
                                    // 已在跟踪列表中，跳过
                                    if proc_list.iter().any(|(n, _)| n == name) {
                                        log::debug!("[{}] 已在守护跟踪列表中，跳过", name);
                                    } else {
                                        let process = FrpcProcess::from_pid(
                                            pid,
                                            name.to_string(),
                                            exe.clone(),
                                            conf.clone(),
                                        );
                                        proc_list.push((name.to_string(), process));
                                        log::info!(
                                            "[{}] UI 通知 TRACK (PID: {})，已纳入守护跟踪",
                                            name,
                                            pid
                                        );
                                    }
                                } else {
                                    log::debug!("[{}] 不在自启动列表中，跳过 TRACK", name);
                                }
                            }
                        }
                    }
                }
            }

            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
        }
    });
}
use windows_service::service::{
    ServiceAccess, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo,
    ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::config;
use crate::frpc_mg::FrpcProcess;

pub const SERVICE_NAME: &str = "FrpcService";
pub const DISPLAY_NAME: &str = "FRP Client Service";
pub const SERVICE_ARG: &str = "--service";

// =========================================================================
//  交互模式入口
// =========================================================================

/// 服务预检查结果
#[derive(Clone, Debug)]
pub(crate) enum PreCheckResult {
    Running,
    Stopped,
    NotRegistered,
}

/// 检查服务状态并启动 GUI
pub fn check_and_run_app() -> Result<()> {
    let pre_check = check_service_status()?;
    crate::app::run_app(pre_check);
    Ok(())
}

/// 检查 Windows 服务当前状态
pub(crate) fn check_service_status() -> Result<PreCheckResult> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    if let Ok(service) = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        let status = service.query_status()?;
        match status.current_state {
            ServiceState::Running => Ok(PreCheckResult::Running),
            ServiceState::Stopped => Ok(PreCheckResult::Stopped),
            _ => Err(anyhow::anyhow!(
                "服务处于非预期状态：{:?}",
                status.current_state
            )),
        }
    } else {
        Ok(PreCheckResult::NotRegistered)
    }
}

// =========================================================================
//  服务注册 / 注销
// =========================================================================

/// 注册 Windows 服务（如果已存在则先删除再重建）
pub(crate) fn install_service() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::all())?;

    // 如果服务已存在，先停止并删除
    if let Ok(service) = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) {
        log::info!("服务 {} 已存在，尝试删除旧服务", SERVICE_NAME);
        stop_service_if_running(&service)?;
        service.delete().context("无法删除旧服务")?;
        std::thread::sleep(Duration::from_millis(500));
    }

    // 创建新服务
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;
    let exe_path = env::current_exe().context("无法获取当前可执行文件路径")?;
    manager
        .create_service(
            &ServiceInfo {
                name: OsString::from(SERVICE_NAME),
                display_name: OsString::from(DISPLAY_NAME),
                service_type: ServiceType::OWN_PROCESS,
                start_type: ServiceStartType::AutoStart,
                error_control: ServiceErrorControl::Normal,
                executable_path: PathBuf::from(&exe_path),
                launch_arguments: vec![OsString::from(SERVICE_ARG)],
                dependencies: vec![],
                account_name: None,
                account_password: None,
            },
            ServiceAccess::all(),
        )
        .context("创建服务失败，请确保以管理员身份运行")?;
    log::info!("服务 {} 已成功注册（重启电脑后生效）", SERVICE_NAME);

    Ok(())
}

/// 注销 Windows 服务（先停止再删除）
pub(crate) fn uninstall_service() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::all())?;
    let service = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    )?;
    stop_service_if_running(&service)?;
    service.delete().context("无法删除服务")?;
    log::info!("服务 {} 已删除", SERVICE_NAME);
    Ok(())
}

/// 启动 Windows 服务
pub(crate) fn start_service() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::START | ServiceAccess::QUERY_STATUS,
    )?;
    let status = service.query_status()?;
    if status.current_state == ServiceState::Running {
        log::info!("服务 {} 已在运行", SERVICE_NAME);
        return Ok(());
    }
    service.start(&[] as &[&str]).context("无法启动服务")?;
    log::info!("服务 {} 已启动", SERVICE_NAME);
    Ok(())
}

/// 停止 Windows 服务
#[allow(dead_code)]
pub(crate) fn stop_service() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
    )?;
    stop_service_if_running(&service)
}

/// 重启 Windows 服务（先停止再启动，不影响已运行的 frpc 进程）
#[allow(dead_code)]
pub(crate) fn restart_service() -> Result<()> {
    stop_service()?;
    std::thread::sleep(Duration::from_millis(500));
    start_service()
}

/// 启动一个 frpc 配置进程（无连接回调）
#[allow(dead_code)]
pub fn start_frpc_process(name: &str) -> Result<FrpcProcess> {
    start_frpc_process_with_sender(name, None)
}

/// 启动一个 frpc 配置进程，可传入连接成功回调
pub fn start_frpc_process_with_sender(
    name: &str,
    on_connected: Option<Sender<()>>,
) -> Result<FrpcProcess> {
    let exe_path = config::frpc_exe_path().context("无法获取 frpc.exe 路径")?;
    let config_path = config::config_toml_path(name).context("无法获取配置文件路径")?;
    FrpcProcess::start(name.to_string(), exe_path, config_path, on_connected)
}

// =========================================================================
//  内部辅助
// =========================================================================

/// 如果服务正在运行则停止它
fn stop_service_if_running(service: &windows_service::service::Service) -> Result<()> {
    let status = service.query_status()?;
    if status.current_state == ServiceState::Stopped {
        return Ok(());
    }
    service.stop().context("无法停止服务")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let status = service.query_status()?;
        if status.current_state == ServiceState::Stopped {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            return Err(anyhow::anyhow!("服务停止超时"));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

// =========================================================================
//  服务模式（由 SCM 启动）
// =========================================================================

extern "system" fn service_main(_arguments: u32, _argv: *mut *mut u16) {
    if let Err(e) = run_service() {
        log::error!("服务运行失败: {:?}", e);
    }
}

pub fn run_service_dispatcher() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, service_main)?;
    Ok(())
}

fn run_service() -> Result<()> {
    SERVICE_STOP_REQUESTED.store(false, Ordering::SeqCst);
    let status_handle =
        service_control_handler::register(SERVICE_NAME, |control_event| match control_event {
            windows_service::service::ServiceControl::Stop
            | windows_service::service::ServiceControl::Shutdown => {
                SERVICE_STOP_REQUESTED.store(true, Ordering::SeqCst);
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        })
        .context("无法注册服务控制处理程序")?;
    set_service_status(&status_handle, ServiceState::StartPending)?;

    let mut settings = config::load_settings();

    // 服务启动时始终启动所有自启动配置（进程守护只负责崩溃后重启）
    // processes 共享给管道线程（TRACK 命令需要添加进程）
    let processes: Arc<Mutex<Vec<(String, FrpcProcess)>>> =
        Arc::new(Mutex::new(start_auto_start_processes()));

    {
        let proc_list = processes.lock().unwrap();
        log::info!(
            "服务已启动，进程守护: {}，已跟踪 {} 个进程",
            settings.process_guard,
            proc_list.len()
        );
    }
    set_service_status(&status_handle, ServiceState::Running)?;

    // 进程守护未开启时：启动自启动配置后立即退出
    // frpc 进程会继续作为孤儿进程运行
    if !settings.process_guard {
        log::info!("进程守护未开启，服务退出（已启动自启动配置）");
        set_service_status(&status_handle, ServiceState::Stopped)?;
        return Ok(());
    }

    // auto_start_map 共享给管道线程（TRACK 命令需要查找 exe/conf）
    let auto_start_map = Arc::new(discover_auto_start_map());

    // 创建跨进程命名事件，UI 可通过信号通知服务
    let guard_event = create_named_event(&guard_event_name(), "进程守护")?;
    let process_changed_event = create_named_event(&process_changed_event_name(), "进程状态变更")?;

    // 通过命名管道接收 UI 的命令（STOP/START/CLEAR/TRACK）
    let guard_stopped: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    start_guard_stopped_pipe(
        Arc::clone(&guard_stopped),
        Arc::clone(&processes),
        Arc::clone(&auto_start_map),
    );

    loop {
        if SERVICE_STOP_REQUESTED.load(Ordering::SeqCst) {
            log::info!("收到服务停止信号");
            unsafe {
                CloseHandle(guard_event);
                CloseHandle(process_changed_event);
            }
            set_service_status(&status_handle, ServiceState::Stopped)?;
            return Ok(());
        }

        // 使用命名事件等待 1 秒，替代 thread::sleep
        // - WAIT_OBJECT_0: guard_event 信号化（进程守护开关切换）
        // - WAIT_TIMEOUT: 超时，继续检查进程状态
        let wait_result = unsafe { WaitForMultipleObjects(1, [guard_event].as_ptr(), 0, 1000) };
        match wait_result {
            WAIT_OBJECT_0 => {
                settings.process_guard = !settings.process_guard;
                log::info!(
                    "收到进程守护变更信号，process_guard={}",
                    settings.process_guard
                );
                if settings.process_guard {
                    // 开启进程守护：清理已在守护关闭期间退出的进程
                    // 只监控开启后存活的进程，避免重启之前已死的进程
                    let mut proc_list = processes.lock().unwrap();
                    let before = proc_list.len();
                    proc_list.retain(|(_, proc)| FrpcProcess::is_pid_running(proc.pid()));
                    let after = proc_list.len();
                    if before != after {
                        log::info!(
                            "进程守护已开启，清理 {} 个已退出进程，当前跟踪 {} 个",
                            before - after,
                            after
                        );
                    } else {
                        log::info!("进程守护已开启，当前跟踪 {} 个进程", after);
                    }
                } else {
                    // 关闭进程守护：退出服务，frpc 进程继续作为孤儿进程运行
                    log::info!("进程守护已关闭，服务退出");
                    unsafe {
                        CloseHandle(guard_event);
                        CloseHandle(process_changed_event);
                    }
                    set_service_status(&status_handle, ServiceState::Stopped)?;
                    return Ok(());
                }
            }
            WAIT_TIMEOUT => {} // 超时，继续检查进程状态
            _ => {
                log::error!("WaitForMultipleObjects 返回未知状态: {}", wait_result);
            }
        }

        // 进程守护开启：检查是否有进程退出并重启
        // Phase 1: 检测已退出的进程，构建重启候选列表
        let mut restart_list = Vec::new();
        {
            let gs = guard_stopped.lock().unwrap();
            let mut proc_list = processes.lock().unwrap();
            proc_list.retain(|(name, proc)| {
                if FrpcProcess::is_pid_running(proc.pid()) {
                    true
                } else {
                    if gs.contains(name) {
                        log::info!("[{}] 进程已退出（UI 手动停止，不重启）", name);
                    } else {
                        // 暂不重启，等 grace period 后再确认
                        log::info!("[{}] 进程已退出，等待确认后重启", name);
                        restart_list.push(name.clone());
                    }
                    false
                }
            });
        }

        // Phase 2: 等待 500ms 给 STOP 命令到达的时间，然后重新检查 guard_stopped
        if !restart_list.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let gs = guard_stopped.lock().unwrap();
            let mut proc_list = processes.lock().unwrap();
            for name in &restart_list {
                if gs.contains(name) {
                    log::info!("[{}] 等待期间收到停止命令，取消重启", name);
                    continue;
                }
                if let Some((exe, conf)) = auto_start_map.get(name) {
                    match FrpcProcess::start(name.clone(), exe.clone(), conf.clone(), None) {
                        Ok(p) => {
                            log::info!("[{}] 进程守护重启成功", name);
                            proc_list.push((name.clone(), p));
                        }
                        Err(e) => log::error!("[{}] 进程守护重启失败: {:?}", name, e),
                    }
                }
            }
            // 通知 UI 更新界面显示
            signal_process_changed();
        }
    }
}

/// 启动所有自启动配置（跳过已运行的），返回进程列表
fn start_auto_start_processes() -> Vec<(String, FrpcProcess)> {
    let running_frpc = discover_running_frpc_processes();
    let instances = match discover_auto_start_instances() {
        Ok(v) => v,
        Err(e) => {
            log::error!("发现自启动配置失败: {:?}", e);
            return Vec::new();
        }
    };
    let mut processes = Vec::new();
    for (id, exe, conf) in instances {
        if let Some((_, pid)) = running_frpc.iter().find(|(n, _)| n == &id) {
            if FrpcProcess::is_pid_running(*pid) {
                let process = FrpcProcess::from_pid(*pid, id.clone(), exe, conf);
                log::info!("[{}] 检测到已运行的进程 (PID: {})", id, pid);
                processes.push((id, process));
                continue;
            }
        }
        match FrpcProcess::start(id.clone(), exe, conf, None) {
            Ok(p) => {
                log::info!("[{}] frpc 进程已启动", id);
                processes.push((id, p));
            }
            Err(e) => log::error!("启动 frpc 实例失败: {:?}", e),
        }
    }
    if processes.is_empty() {
        log::warn!("没有任何 frpc 进程成功启动");
    } else {
        log::info!("成功启动 {} 个 frpc 实例", processes.len());
    }
    processes
}

fn set_service_status(
    handle: &windows_service::service_control_handler::ServiceStatusHandle,
    state: ServiceState,
) -> Result<()> {
    let mut controls = ServiceControlAccept::empty();
    if state == ServiceState::Running {
        controls = ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN;
    }
    handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: controls,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::ZERO,
        process_id: None,
    })?;
    Ok(())
}

fn discover_auto_start_instances() -> Result<Vec<(String, PathBuf, PathBuf)>> {
    let frpc_exe = config::frpc_exe_path().context("无法获取 frpc.exe 路径")?;
    if !frpc_exe.exists() {
        return Ok(Vec::new());
    }
    let mut instances = Vec::new();
    for meta in config::get_auto_start_configs().unwrap_or_default() {
        let conf = config::config_toml_path(&meta.name)?;
        if conf.exists() {
            instances.push((meta.name.clone(), frpc_exe.clone(), conf));
        }
    }
    Ok(instances)
}

/// 发现自启动配置，返回 name -> (exe, conf) 的映射
fn discover_auto_start_map() -> std::collections::HashMap<String, (PathBuf, PathBuf)> {
    let mut map = std::collections::HashMap::new();
    let frpc_exe = match config::frpc_exe_path() {
        Ok(p) if p.exists() => p,
        _ => return map,
    };
    for meta in config::get_auto_start_configs().unwrap_or_default() {
        if let Ok(conf) = config::config_toml_path(&meta.name) {
            if conf.exists() {
                map.insert(meta.name.clone(), (frpc_exe.clone(), conf));
            }
        }
    }
    map
}

/// 发现当前正在运行的 frpc 进程，匹配到已有配置
///
/// 返回 (配置名, PID) 的列表。优先使用 wmic（快速），失败则回退到 PowerShell。
pub fn discover_running_frpc_processes() -> Vec<(String, u32)> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let frpc_exe = match config::frpc_exe_path() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    if !frpc_exe.exists() {
        return Vec::new();
    }

    let configs = config::load_configs().unwrap_or_default();
    if configs.is_empty() {
        return Vec::new();
    }
    let conf_dir = config::conf_dir().unwrap_or_default();

    // 尝试 wmic（快速），失败或无输出则回退到 PowerShell
    let stdout = match std::process::Command::new("wmic")
        .args([
            "process",
            "where",
            "name='frpc.exe'",
            "get",
            "ProcessId,CommandLine",
            "/FORMAT:CSV",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) if !o.stdout.is_empty() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => {
            log::debug!("wmic 不可用或无输出，尝试 PowerShell");
            match std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command",
                    "Get-CimInstance Win32_Process -Filter \"Name='frpc.exe'\" | Select-Object ProcessId,CommandLine | ConvertTo-CSV -NoTypeInformation"])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
            {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
                _ => {
                    log::warn!("PowerShell 也无法获取进程信息");
                    return Vec::new();
                }
            }
        }
    };

    if stdout.trim().is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let fields = parse_csv_line(line);

        // 跳过标题行
        if fields
            .iter()
            .any(|f| f == "ProcessId" || f == "CommandLine" || f == "Node")
        {
            continue;
        }

        // 解析 PID 和命令行（wmic: Node,CommandLine,ProcessId；PowerShell: ProcessId,CommandLine）
        let (pid, cmd_line) = if fields.len() >= 3 {
            match fields[2].trim().parse::<u32>() {
                Ok(p) => (p, fields[1].as_str()),
                Err(_) => continue,
            }
        } else if fields.len() == 2 {
            if let Ok(p) = fields[0].trim().parse::<u32>() {
                (p, fields[1].as_str())
            } else if let Ok(p) = fields[1].trim().parse::<u32>() {
                (p, fields[0].as_str())
            } else {
                continue;
            }
        } else {
            continue;
        };

        // 匹配配置
        for config_meta in &configs {
            let config_path = conf_dir.join(format!("{}.toml", config_meta.name));
            let config_path_str = config_path.to_string_lossy();
            if cmd_line.contains(&*config_path_str) {
                result.push((config_meta.name.clone(), pid));
                break;
            }
        }
    }

    result
}

/// CSV 行解析，支持引号包裹的字段
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current.trim().to_string());
    fields
}
