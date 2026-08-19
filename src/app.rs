//! 主应用视图：AppView 结构体、事件处理、run_app 入口

use anyhow::Result;
use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Entity, SharedString, Task, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};
use gpui_component::dialog::DialogButtonProps;
use gpui_component::input::{EditorState, InputState};
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::{ActiveTheme, IndexPath, Root, WindowExt};
use log::LevelFilter;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config::{self, FrpcConfigMeta};
use crate::download;
use crate::frpc_mg::FrpcProcess;
use crate::logger;
use crate::message::MessageLevel;
use crate::pages;
use crate::service::{self, PreCheckResult};
use crate::sidebar;
use crate::theme;

/// 自定义暗色主题 JSON
/// 当前页面
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Page {
    ConfigList,
    ConfigEditor { original_name: Option<String> },
    Settings,
}

/// 运行中的进程信息
pub(crate) struct RunningProcess {
    pub process: FrpcProcess,
}

/// 主界面视图
pub(crate) struct AppView {
    pub page: Page,
    pub service_registered: bool,
    pub configs: Vec<FrpcConfigMeta>,
    pub running: HashMap<String, RunningProcess>,
    pub stopped_configs: std::collections::HashSet<String>, // 手动停止的配置，防止被健康检查重新拉起
    pub edit_name: String,
    pub edit_content: String,
    pub edit_auto_start: bool,
    pub name_input: Entity<InputState>,
    pub content_input: Entity<EditorState>,
    pub frpc_version: Option<String>,
    pub is_checking_update: bool,
    pub is_downloading: bool,
    pub download_percent: u64,
    pub is_processing: bool,
    pub status_message: Option<String>,
    pub status_level: MessageLevel,
    pub config_page: usize,
    pub theme_select: Entity<SelectState<Vec<SharedString>>>,
    pub log_level_select: Entity<SelectState<Vec<SharedString>>>,
    pub process_guard: bool,
}

impl AppView {
    pub fn new(
        pre_check: PreCheckResult,
        name_input: Entity<InputState>,
        content_input: Entity<EditorState>,
        theme_select: Entity<SelectState<Vec<SharedString>>>,
        log_level_select: Entity<SelectState<Vec<SharedString>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let configs = config::load_configs().unwrap_or_default();
        let service_registered = !matches!(pre_check, PreCheckResult::NotRegistered);

        // 恢复上次运行的 frpc 进程状态
        let mut running = HashMap::new();
        let frpc_exe = config::frpc_exe_path().ok().filter(|p| p.exists());
        if let Some(exe_path) = frpc_exe {
            for (name, pid) in service::discover_running_frpc_processes() {
                if FrpcProcess::is_pid_running(pid) {
                    let config_path = config::config_toml_path(&name).unwrap_or_default();
                    let process =
                        FrpcProcess::from_pid(pid, name.clone(), exe_path.clone(), config_path);
                    running.insert(name.clone(), RunningProcess { process });
                    log::info!("恢复 frpc 进程状态: {} (PID: {})", name, pid);
                }
            }
        }

        let s = Self {
            page: Page::ConfigList,
            service_registered,
            configs,
            running,
            stopped_configs: std::collections::HashSet::new(),
            edit_name: String::new(),
            edit_content: String::new(),
            edit_auto_start: false,
            name_input,
            content_input,
            frpc_version: None,
            is_checking_update: false,
            is_downloading: false,
            download_percent: 0,
            is_processing: false,
            status_message: None,
            status_level: MessageLevel::Info,
            config_page: 0,
            theme_select: theme_select.clone(),
            log_level_select: log_level_select.clone(),
            process_guard: config::load_settings().process_guard,
        };

        // 订阅主题下拉选择事件
        cx.subscribe_in(&theme_select, window, |view, _entity, event, window, cx| {
            view.on_theme_selected(event, window, cx);
        })
        .detach();

        // 订阅日志级别下拉选择事件
        cx.subscribe_in(
            &log_level_select,
            window,
            |view, _entity, event, window, cx| {
                view.on_log_level_selected(event, window, cx);
            },
        )
        .detach();

        s
    }

    pub fn switch_page(&mut self, page: Page, _cx: &mut Context<Self>) {
        self.page = page;
        self.status_message = None;
        self.config_page = 0;
    }

    pub fn toggle_process_guard(&mut self, cx: &mut Context<Self>) {
        if !self.service_registered {
            self.set_status_message(
                "请先注册服务后再开启进程守护".to_string(),
                MessageLevel::Warning,
                cx,
            );
            return;
        }
        self.process_guard = !self.process_guard;
        // 保留其他设置（如日志级别）
        let settings = config::AppSettings {
            process_guard: self.process_guard,
            ..config::load_settings()
        };
        match config::save_settings(&settings) {
            Ok(()) => {
                if self.process_guard {
                    // 开启进程守护：启动 Service，Service 读取设置后持续运行监控
                    match service::start_service() {
                        Ok(()) => {
                            self.set_status_message(
                                "进程守护已开启，服务已启动".to_string(),
                                MessageLevel::Success,
                                cx,
                            );
                        }
                        Err(e) => {
                            self.set_status_message(
                                format!("启动服务失败: {}", e),
                                MessageLevel::Error,
                                cx,
                            );
                        }
                    }
                } else {
                    // 关闭进程守护：通知 Service 退出
                    service::signal_guard_changed();
                    self.set_status_message(
                        "进程守护已关闭".to_string(),
                        MessageLevel::Success,
                        cx,
                    );
                }
                log::info!("进程守护设置已变更: {}", self.process_guard);
            }
            Err(e) => {
                log::error!("保存进程守护设置失败: {}", e);
                self.set_status_message(format!("保存设置失败: {}", e), MessageLevel::Error, cx);
            }
        }
        cx.notify();
    }

    pub fn on_theme_selected(
        &mut self,
        event: &SelectEvent<Vec<SharedString>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let SelectEvent::Confirm(Some(name)) = event {
            let name = name.to_string();
            theme::apply_theme(&name, cx);
            theme::save_theme_preference(&name);
            self.set_status_message(
                format!("主题已切换为 '{}'", name),
                MessageLevel::Success,
                cx,
            );
        }
    }

    pub fn on_log_level_selected(
        &mut self,
        event: &SelectEvent<Vec<SharedString>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let SelectEvent::Confirm(Some(level)) = event {
            let level_str = level.to_string().to_lowercase();
            let filter = match level_str.as_str() {
                "off" => LevelFilter::Off,
                "error" => LevelFilter::Error,
                "warn" => LevelFilter::Warn,
                "debug" => LevelFilter::Debug,
                "trace" => LevelFilter::Trace,
                _ => LevelFilter::Info,
            };
            match logger::set_log_level(filter) {
                Ok(()) => {
                    // 持久化设置
                    let mut settings = config::load_settings();
                    settings.log_level = level_str.clone();
                    match config::save_settings(&settings) {
                        Ok(()) => {
                            self.set_status_message(
                                format!("日志级别已切换为 '{}'", level_str),
                                MessageLevel::Success,
                                cx,
                            );
                        }
                        Err(e) => {
                            log::error!("保存日志级别设置失败: {}", e);
                            self.set_status_message(
                                format!("保存设置失败：{}", e),
                                MessageLevel::Error,
                                cx,
                            );
                        }
                    }
                }
                Err(e) => {
                    log::error!("切换日志级别失败: {}", e);
                    self.set_status_message(
                        format!("切换日志级别失败：{}", e),
                        MessageLevel::Error,
                        cx,
                    );
                }
            }
        }
    }

    pub fn switch_page_with_message(
        &mut self,
        page: Page,
        msg: String,
        level: MessageLevel,
        cx: &mut Context<Self>,
    ) {
        self.page = page;
        self.status_message = Some(msg);
        self.status_level = level;
        self.config_page = 0;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async { std::thread::sleep(std::time::Duration::from_secs(3)) })
                .await;
            this.update(cx, |v, cx| {
                v.status_message = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn set_status_message(&mut self, msg: String, level: MessageLevel, cx: &mut Context<Self>) {
        self.status_message = Some(msg);
        self.status_level = level;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async { std::thread::sleep(std::time::Duration::from_secs(3)) })
                .await;
            this.update(cx, |v, cx| {
                v.status_message = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn reload_configs(&mut self, cx: &mut Context<Self>) {
        self.configs = config::load_configs().unwrap_or_default();
        let total_pages = (self.configs.len() + 7) / 8;
        if self.config_page > 0 && self.config_page >= total_pages {
            self.config_page = total_pages.saturating_sub(1);
        }
        cx.notify();
    }

    pub fn open_add_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_name = String::new();
        self.edit_content = String::new();
        self.edit_auto_start = true;
        self.status_message = None;
        self.name_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.content_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.switch_page(
            Page::ConfigEditor {
                original_name: None,
            },
            cx,
        );
    }

    pub fn open_edit_config(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_name = name.to_string();
        self.edit_content = config::read_config_content(name).unwrap_or_default();
        self.edit_auto_start = self
            .configs
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.auto_start)
            .unwrap_or(false);
        self.status_message = None;
        self.name_input
            .update(cx, |s, cx| s.set_value(name, window, cx));
        self.content_input.update(cx, |s, cx| {
            s.set_value(&self.edit_content.clone(), window, cx)
        });
        self.switch_page(
            Page::ConfigEditor {
                original_name: Some(name.to_string()),
            },
            cx,
        );
    }

    pub fn save_config(&mut self, cx: &mut Context<Self>) {
        self.edit_name = self.name_input.read(cx).value().to_string();
        self.edit_content = self.content_input.read(cx).value().to_string();
        let name = self.edit_name.trim().to_string();
        if name.is_empty() {
            self.set_status_message("配置名称不能为空".to_string(), MessageLevel::Error, cx);
            return;
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            self.set_status_message(
                "配置名称只能包含字母、数字、下划线和连字符".to_string(),
                MessageLevel::Error,
                cx,
            );
            return;
        }
        if self.edit_content.trim().is_empty() {
            self.set_status_message("配置内容不能为空".to_string(), MessageLevel::Error, cx);
            return;
        }
        if let Page::ConfigEditor {
            original_name: ref orig,
        } = self.page
        {
            if orig.is_none() && config::config_exists(&name) {
                self.set_status_message(format!("配置 '{}' 已存在", name), MessageLevel::Error, cx);
                return;
            }
        }
        if let Page::ConfigEditor {
            original_name: Some(ref orig),
        } = self.page
        {
            if orig != &name {
                let _ = config::delete_config(orig);
            }
        }
        // 验证 TOML 格式并提取配置信息
        let (server_addr, proxies) = match config::validate_toml(&self.edit_content) {
            Ok(result) => result,
            Err(e) => {
                self.set_status_message(format!("保存失败：{}", e), MessageLevel::Error, cx);
                return;
            }
        };
        match config::save_config(
            &name,
            &self.edit_content,
            self.edit_auto_start,
            &server_addr,
            proxies,
        ) {
            Ok(()) => {
                self.reload_configs(cx);
                self.switch_page_with_message(
                    Page::ConfigList,
                    format!("配置 '{}' 保存成功", name),
                    MessageLevel::Success,
                    cx,
                );
            }
            Err(e) => {
                self.set_status_message(format!("保存失败：{}", e), MessageLevel::Error, cx);
            }
        }
    }

    pub fn delete_config(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(mut rp) = self.running.remove(name) {
            let _ = rp.process.stop();
        }
        match config::delete_config(name) {
            Ok(()) => {
                log::info!("配置 '{}' 已删除", name);
                self.reload_configs(cx);
                self.set_status_message(
                    format!("配置 '{}' 已删除", name),
                    MessageLevel::Success,
                    cx,
                );
            }
            Err(e) => {
                log::error!("删除配置 '{}' 失败: {}", name, e);
                self.set_status_message(format!("删除失败：{}", e), MessageLevel::Error, cx);
            }
        }
    }

    /// 复制配置：以源配置的 toml 内容创建一个新配置，名称自动追加 -copy 后缀
    pub fn duplicate_config(&mut self, name: &str, cx: &mut Context<Self>) {
        let content = match config::read_config_content(name) {
            Ok(c) => c,
            Err(e) => {
                log::error!("复制配置 '{}' 失败: {}", name, e);
                self.set_status_message(format!("复制失败：{}", e), MessageLevel::Error, cx);
                return;
            }
        };

        // 继承源配置的元数据（自启动、服务器地址、代理列表）
        let source_meta = self.configs.iter().find(|c| c.name == name).cloned();
        let auto_start = source_meta.as_ref().map(|m| m.auto_start).unwrap_or(false);
        let server_addr = source_meta
            .as_ref()
            .map(|m| m.server_addr.clone())
            .unwrap_or_default();
        let proxies = source_meta.map(|m| m.proxies).unwrap_or_default();

        // 生成不重复的新名称：xxx-copy、xxx-copy-2、xxx-copy-3 ...
        let existing: Vec<String> = self.configs.iter().map(|c| c.name.clone()).collect();
        let mut new_name = format!("{}-copy", name);
        if existing.contains(&new_name) {
            let mut i = 2;
            while existing.contains(&format!("{}-copy-{}", name, i)) {
                i += 1;
            }
            new_name = format!("{}-copy-{}", name, i);
        }

        match config::save_config(&new_name, &content, auto_start, &server_addr, proxies) {
            Ok(()) => {
                log::info!("配置 '{}' 已复制为 '{}'", name, new_name);
                self.reload_configs(cx);
                self.set_status_message(
                    format!("已复制配置为 '{}'", new_name),
                    MessageLevel::Success,
                    cx,
                );
            }
            Err(e) => {
                log::error!("复制配置 '{}' 失败: {}", name, e);
                self.set_status_message(format!("复制失败：{}", e), MessageLevel::Error, cx);
            }
        }
    }

    pub fn start_config(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.running.contains_key(name) {
            return;
        }
        // 服务已注册时，先检查是否已被 Service 管理（避免重复启动）
        if self.service_registered {
            let running_frpc = service::discover_running_frpc_processes();
            if let Some((_, pid)) = running_frpc.iter().find(|(n, _)| n == name) {
                if FrpcProcess::is_pid_running(*pid) {
                    let frpc_exe = config::frpc_exe_path().ok().filter(|p| p.exists());
                    if let Some(exe_path) = frpc_exe {
                        let config_path = config::config_toml_path(name).unwrap_or_default();
                        let process =
                            FrpcProcess::from_pid(*pid, name.to_string(), exe_path, config_path);
                        self.running
                            .insert(name.to_string(), RunningProcess { process });
                        log::info!("[{}] 进程已由 Service 管理，同步状态 (PID: {})", name, pid);
                        cx.notify();
                        return;
                    }
                }
            }
        }
        // 从手动停止列表中移除，通过命名管道通知 Service
        self.stopped_configs.remove(name);
        service::send_guard_stopped_command(&format!("START:{}", name));
        // 检查 frpc.exe 是否存在
        if !crate::download::has_frpc_executable(
            &std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
        ) {
            self.set_status_message(
                "请先在设置中下载 frpc 程序".to_string(),
                MessageLevel::Warning,
                cx,
            );
            return;
        }
        let n = name.to_string();
        self.is_processing = true;
        self.status_message = None;
        cx.notify();

        // 创建通道用于检测 frpc 连接成功
        let (tx, rx) = std::sync::mpsc::channel();

        let task: Task<Result<FrpcProcess>> = cx
            .background_spawn(async move { service::start_frpc_process_with_sender(&n, Some(tx)) });
        let nc = name.to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |view, cx| {
                view.is_processing = false;
                match result {
                    Ok(p) => {
                        log::info!("[{}] frpc 进程已启动", nc);
                        // 通知 Service 纳入守护跟踪（仅服务已注册时）
                        if view.service_registered {
                            service::send_guard_stopped_command(&format!(
                                "TRACK:{}:{}",
                                nc,
                                p.pid()
                            ));
                        }
                        view.running
                            .insert(nc.clone(), RunningProcess { process: p });
                        cx.notify();

                        // 启动后台任务监听连接成功
                        let name_for_toast = nc.clone();
                        cx.spawn(async move |this, cx| {
                            // 在后台线程等待连接成功信号
                            let connected = cx
                                .background_spawn(async move {
                                    rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok()
                                })
                                .await;
                            if connected {
                                this.update(cx, |view, cx| {
                                    if view.running.contains_key(&name_for_toast) {
                                        view.set_status_message(
                                            format!("'{}' 连接成功", name_for_toast),
                                            MessageLevel::Success,
                                            cx,
                                        );
                                    }
                                })
                                .ok();
                            }
                        })
                        .detach();

                        // 500ms 后检查进程是否立即退出（如配置解析错误）
                        let name_check = nc.clone();
                        cx.spawn(async move |this, cx| {
                            cx.background_spawn(async {
                                std::thread::sleep(Duration::from_millis(500));
                            })
                            .await;
                            this.update(cx, |view, cx| {
                                if let Some(rp) = view.running.get_mut(&name_check) {
                                    if let Some(status) = rp.process.check_exit_status() {
                                        log::error!(
                                            "[{}] frpc 启动后立即退出，退出码: {}",
                                            name_check,
                                            status
                                        );
                                        view.running.remove(&name_check);
                                        view.set_status_message(
                                            format!(
                                                "'{}' 启动失败，请检查配置是否正确 (退出码: {})",
                                                name_check, status
                                            ),
                                            MessageLevel::Error,
                                            cx,
                                        );
                                    }
                                }
                            })
                            .ok();
                        })
                        .detach();
                    }
                    Err(e) => {
                        log::error!("[{}] 启动失败: {}", nc, e);
                        view.set_status_message(
                            format!("启动失败：{}", e),
                            MessageLevel::Error,
                            cx,
                        );
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn stop_config(&mut self, name: &str, cx: &mut Context<Self>) {
        // 标记为手动停止，通过命名管道通知 Service 不要重启
        self.stopped_configs.insert(name.to_string());
        service::send_guard_stopped_command(&format!("STOP:{}", name));
        if let Some(mut rp) = self.running.remove(name) {
            self.is_processing = true;
            cx.notify();
            let task: Task<Result<()>> = cx.background_spawn(async move {
                rp.process.stop()?;
                Ok(())
            });
            let nc = name.to_string();
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |view, cx| {
                    view.is_processing = false;
                    match result {
                        Ok(()) => {
                            log::info!("[{}] frpc 已停止", nc);
                            view.set_status_message(
                                format!("'{}'已停止", nc),
                                MessageLevel::Success,
                                cx,
                            );
                        }
                        Err(e) => {
                            log::error!("[{}] 停止失败: {}", nc, e);
                            view.set_status_message(
                                format!("停止失败：{}", e),
                                MessageLevel::Error,
                                cx,
                            );
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// 重启配置：先停止当前进程，停止成功后再重新启动
    pub fn restart_config(&mut self, name: &str, cx: &mut Context<Self>) {
        // 未在运行则直接启动
        if !self.running.contains_key(name) {
            self.start_config(name, cx);
            return;
        }

        // 先标记停止并通知 Service，避免守护在重启间隙自动拉起进程
        self.stopped_configs.insert(name.to_string());
        service::send_guard_stopped_command(&format!("STOP:{}", name));
        if let Some(mut rp) = self.running.remove(name) {
            self.is_processing = true;
            self.status_message = None;
            cx.notify();
            let task: Task<Result<()>> = cx.background_spawn(async move {
                rp.process.stop()?;
                Ok(())
            });
            let nc = name.to_string();
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |view, cx| {
                    view.is_processing = false;
                    match result {
                        Ok(()) => {
                            log::info!("[{}] frpc 已停止，准备重启", nc);
                            // 清除停止标记并重新启动
                            view.stopped_configs.remove(&nc);
                            service::send_guard_stopped_command(&format!("START:{}", nc));
                            view.start_config(&nc, cx);
                        }
                        Err(e) => {
                            log::error!("[{}] 重启时停止失败: {}", nc, e);
                            // 恢复状态：取消停止标记，避免守护行为与实际不一致
                            view.stopped_configs.remove(&nc);
                            service::send_guard_stopped_command(&format!("START:{}", nc));
                            view.set_status_message(
                                format!("'{}' 重启失败：{}", nc, e),
                                MessageLevel::Error,
                                cx,
                            );
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    pub fn start_download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_checking_update = true;
        self.is_downloading = false;
        self.download_percent = 0;
        self.status_message = None;
        cx.notify();

        // 记录窗口句柄，供异步回调中打开确认弹窗
        let window_handle = window.window_handle();

        // 第一步：在后台检查版本
        let check_task: Task<Result<Option<String>>> =
            cx.background_spawn(async move { download::check_update() });

        cx.spawn(async move |this, cx| {
            let result = check_task.await;
            let has_update = this
                .update(cx, |view, cx| {
                    view.is_checking_update = false;
                    match result {
                        Ok(Some(tag)) => {
                            log::info!("发现新版本: {}", tag);
                            cx.notify();
                            true
                        }
                        Ok(None) => {
                            view.set_status_message(
                                "已经是最新版本".to_string(),
                                MessageLevel::Success,
                                cx,
                            );
                            false
                        }
                        Err(e) => {
                            log::error!("检查版本失败: {}", e);
                            view.set_status_message(
                                format!("检查版本失败：{}", e),
                                MessageLevel::Error,
                                cx,
                            );
                            false
                        }
                    }
                })
                .unwrap_or(false);

            if !has_update {
                return;
            }

            // 有更新：若 frpc 正在运行，先弹窗确认（确认后才停止 frpc 并下载）
            let has_running = this
                .update(cx, |view, _| {
                    !view.running.is_empty()
                        || !service::discover_running_frpc_processes().is_empty()
                })
                .unwrap_or(false);

            if has_running {
                let entity = this;
                let _ = cx.update_window(window_handle, |_root, window, cx| {
                    window.open_alert_dialog(cx, move |alert, _window, _cx| {
                        let entity = entity.clone();
                        alert
                            .title("停止 frpc 并更新？")
                            .description(
                                "检测到 frpc 正在运行，更新前需要先停止 frpc 进程，更新完成后会自动重启。是否继续？",
                            )
                            .button_props(
                                DialogButtonProps::default()
                                    .ok_text("停止并更新")
                                    .cancel_text("取消")
                                    .show_cancel(true),
                            )
                            .on_ok(move |_event, _window, cx| {
                                let _ = entity.update(cx, |view, cx| view.do_download(cx));
                                true
                            })
                    });
                });
            } else {
                // frpc 未运行，直接下载
                this.update(cx, |view, cx| view.do_download(cx)).ok();
            }
        })
        .detach();
    }

    /// 执行实际下载：先停止运行中的 frpc，下载解压后再自动重启
    fn do_download(&mut self, cx: &mut Context<Self>) {
        self.is_downloading = true;
        cx.notify();

        // 更新前先停止所有运行中的 frpc：Windows 下运行中的 frpc.exe 会被锁定，
        // 无法被替换，导致解压失败。记录被停止的配置名，更新完成后自动重启。
        let to_restart: Vec<String> = {
            let mut names: Vec<String> = Vec::new();
            // 1. 由 UI 管理的进程
            for (name, mut rp) in self.running.drain() {
                self.stopped_configs.insert(name.clone());
                service::send_guard_stopped_command(&format!("STOP:{}", name));
                if let Err(e) = rp.process.stop() {
                    log::error!("[{}] 更新前停止 frpc 失败: {}", name, e);
                }
                names.push(name);
            }
            // 2. 由服务或外部启动、未被 UI 跟踪的 frpc 进程
            for (name, pid) in service::discover_running_frpc_processes() {
                if !names.contains(&name) {
                    self.stopped_configs.insert(name.clone());
                    service::send_guard_stopped_command(&format!("STOP:{}", name));
                    if let Err(e) = FrpcProcess::kill_pid(pid) {
                        log::error!("[{}] 更新前终止 frpc 失败 (PID: {}): {}", name, pid, e);
                    }
                    names.push(name);
                }
            }
            names
        };

        // 第二步：启动下载
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let progress = Arc::new(AtomicU64::new(0));
        let pc = progress.clone();

        // 启动进度更新循环
        cx.spawn(async move |this, cx| loop {
            cx.background_spawn(async {
                std::thread::sleep(Duration::from_millis(200));
            })
            .await;
            let ok = this
                .update(cx, |v, cx| {
                    if v.is_downloading {
                        v.download_percent = pc.load(Ordering::Relaxed);
                        cx.notify();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !ok {
                break;
            }
        })
        .detach();

        // 在后台执行下载
        let download_task: Task<Result<()>> = cx.background_spawn(async move {
            download::download_and_extract_frpc(&exe_dir, &move |d, t| {
                progress.store(
                    if t > 0 { (d * 100 / t).min(100) } else { 0 },
                    Ordering::Relaxed,
                );
            })
        });

        // 下载完成，更新 UI 并重启之前停止的配置
        cx.spawn(async move |this, cx| {
            let download_result = download_task.await;
            this.update(cx, |view, cx| {
                view.is_downloading = false;
                match download_result {
                    Ok(()) => {
                        view.set_status_message(
                            "frpc 更新成功".to_string(),
                            MessageLevel::Success,
                            cx,
                        );
                        view.detect_frpc_version(cx);
                    }
                    Err(e) => {
                        log::error!("更新失败: {}", e);
                        view.set_status_message(
                            format!("更新失败：{}", e),
                            MessageLevel::Error,
                            cx,
                        );
                    }
                }
                // 无论更新成功与否，都恢复更新前运行中的 frpc
                for name in &to_restart {
                    view.stopped_configs.remove(name);
                    service::send_guard_stopped_command(&format!("START:{}", name));
                    view.start_config(name, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn detect_frpc_version(&mut self, cx: &mut Context<Self>) {
        let exe_path = match config::frpc_exe_path().ok().filter(|p| p.exists()) {
            Some(p) => p,
            None => {
                self.frpc_version = None;
                cx.notify();
                return;
            }
        };
        let task: Task<Result<String>> = cx.background_spawn(async move {
            let mut cmd = std::process::Command::new(&exe_path);
            cmd.arg("--version");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            let out = cmd.output().map_err(|e| anyhow::anyhow!("{}", e))?;
            let s = String::from_utf8_lossy(&out.stdout);
            let e = String::from_utf8_lossy(&out.stderr);
            Ok(if !s.trim().is_empty() {
                s.trim().to_string()
            } else if !e.trim().is_empty() {
                e.trim().to_string()
            } else {
                "未知版本".to_string()
            })
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |v, cx| {
                v.frpc_version = Some(match r {
                    Ok(v) => v,
                    Err(_) => "无法运行".to_string(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn install_service(&mut self, cx: &mut Context<Self>) {
        self.is_processing = true;
        self.status_message = None;
        cx.notify();
        let task: Task<Result<()>> = cx.background_spawn(async move { service::install_service() });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |v, cx| {
                v.is_processing = false;
                match r {
                    Ok(()) => {
                        v.service_registered = true;

                        // 注册服务后清空手动停止列表
                        // Service 不会立即启动，重启电脑后才生效
                        v.stopped_configs.clear();

                        v.set_status_message(
                            "注册成功，重启电脑后生效".to_string(),
                            MessageLevel::Success,
                            cx,
                        );
                    }
                    Err(e) => {
                        v.set_status_message(format!("注册失败：{}", e), MessageLevel::Error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn uninstall_service(&mut self, cx: &mut Context<Self>) {
        self.is_processing = true;
        self.status_message = None;
        cx.notify();
        let task: Task<Result<()>> =
            cx.background_spawn(async move { service::uninstall_service() });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            this.update(cx, |v, cx| {
                v.is_processing = false;
                match r {
                    Ok(()) => {
                        v.service_registered = false;

                        // 关闭进程守护
                        v.process_guard = false;
                        let settings = config::AppSettings {
                            process_guard: false,
                            ..config::load_settings()
                        };
                        if let Err(e) = config::save_settings(&settings) {
                            log::error!("保存进程守护设置失败: {}", e);
                        }

                        // 清空手动停止列表，通过命名管道通知 Service
                        v.stopped_configs.clear();
                        service::send_guard_stopped_command("CLEAR");

                        // 注销后重新发现仍在运行的进程（服务注销不会停止 frpc）
                        let frpc_exe = config::frpc_exe_path().ok().filter(|p| p.exists());
                        if let Some(exe_path) = frpc_exe {
                            for (name, pid) in service::discover_running_frpc_processes() {
                                if FrpcProcess::is_pid_running(pid)
                                    && !v.running.contains_key(&name)
                                {
                                    let config_path =
                                        config::config_toml_path(&name).unwrap_or_default();
                                    let process = FrpcProcess::from_pid(
                                        pid,
                                        name.clone(),
                                        exe_path.clone(),
                                        config_path,
                                    );
                                    v.running.insert(name.clone(), RunningProcess { process });
                                    log::info!(
                                        "[{}] 注销后发现仍在运行的进程 (PID: {})",
                                        name,
                                        pid
                                    );
                                }
                            }
                        }

                        v.set_status_message("已注销".to_string(), MessageLevel::Success, cx);
                    }
                    Err(e) => {
                        v.set_status_message(format!("注销失败：{}", e), MessageLevel::Error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 启动周期性健康检查，每 3 秒检测所有运行中的 frpc 进程
    /// 服务已注册时，每 9 秒发现一次 Service 管理的进程（减少 wmic 调用频率）
    pub fn start_health_monitor(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut discover_tick: u32 = 0;
            loop {
                // 等待进程状态变更事件，超时 3 秒后执行健康检查
                // 如果 Service 重启了进程，事件会被立即信号化，UI 马上更新
                let signaled = cx
                    .background_spawn(async { service::wait_process_changed(3000) })
                    .await;
                if signaled {
                    // Service 重启了进程，立即触发发现
                    discover_tick = 3;
                }
                let alive = this
                    .update(cx, |view, cx| {
                        // Step 1: 检查已跟踪的进程是否仍然存活
                        let mut dead_names = Vec::new();
                        for (name, rp) in view.running.iter_mut() {
                            if !rp.process.is_running() {
                                log::warn!("[{}] 健康检查发现进程已退出", name);
                                dead_names.push(name.clone());
                            }
                        }

                        // Step 2: 移除已退出的进程（进程守护由 Service 负责，UI 不重启）
                        if !dead_names.is_empty() {
                            for name in &dead_names {
                                view.running.remove(name);
                                log::info!("[{}] 进程已退出，已从运行列表移除", name);
                            }
                            cx.notify();
                        }

                        // Step 3: 服务已注册时，定期发现 Service 管理的进程并同步
                        // 每 3 次健康检查（9秒）执行一次发现，减少 wmic 调用频率
                        if view.service_registered {
                            discover_tick += 1;
                            if discover_tick >= 3 {
                                discover_tick = 0;
                                let running_frpc = service::discover_running_frpc_processes();
                                let frpc_exe = config::frpc_exe_path().ok().filter(|p| p.exists());
                                if let Some(exe_path) = frpc_exe {
                                    let mut changed = false;
                                    for (name, pid) in running_frpc {
                                        // 不检查 stopped_configs：如果 Service 已拉起进程，UI 应显示
                                        if FrpcProcess::is_pid_running(pid)
                                            && !view.running.contains_key(&name)
                                        {
                                            let config_path =
                                                config::config_toml_path(&name).unwrap_or_default();
                                            let process = FrpcProcess::from_pid(
                                                pid,
                                                name.clone(),
                                                exe_path.clone(),
                                                config_path,
                                            );
                                            view.running
                                                .insert(name.clone(), RunningProcess { process });
                                            // 发现新进程时同步清除 stopped_configs
                                            view.stopped_configs.remove(&name);
                                            log::info!(
                                                "[{}] 发现 Service 管理的进程 (PID: {})",
                                                name,
                                                pid
                                            );
                                            changed = true;
                                        }
                                    }
                                    if changed {
                                        cx.notify();
                                    }
                                }
                            }
                        }

                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }
}

impl gpui::Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sb = sidebar::render(self, cx);
        let content = match &self.page {
            Page::ConfigList => pages::config_list::render(self, cx),
            Page::ConfigEditor { .. } => pages::config_editor::render(self, cx),
            Page::Settings => pages::settings::render(self, cx),
        };
        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(cx.theme().background)
            .child(sb)
            .child(div().w(px(1.0)).h_full().bg(cx.theme().border))
            .child(content)
            // Dialog 等悬浮层需要由根视图手动挂载，否则弹窗不会显示
            .children(Root::render_dialog_layer(window, cx))
    }
}

impl Drop for AppView {
    fn drop(&mut self) {
        // 程序退出时通过命名管道通知 Service 清空手动停止列表
        service::send_guard_stopped_command("CLEAR");
        log::info!("程序退出，已通知 Service 清空手动停止列表");
    }
}

pub fn run_app(pre_check: PreCheckResult) {
    let app = gpui_platform::application().with_assets(crate::icons::AppAssets);
    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        theme::load_all_themes(cx);
        let saved_theme = theme::load_theme_preference();
        theme::apply_theme(&saved_theme, cx);
        let bounds = Bounds::centered(None, size(px(930.0), px(600.0)), cx);
        let init = pre_check.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(930.0), px(720.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("FrpDesk")),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let name_input = cx.new(|cx| InputState::new(window, cx));
                let content_input = cx.new(|cx| EditorState::new(window, cx).language("toml"));

                // 创建主题下拉选择
                let themes = theme::available_themes();
                let current = theme::current_theme_name();
                let theme_names: Vec<SharedString> =
                    themes.iter().map(|t| t.name.clone().into()).collect();
                let selected = themes.iter().position(|t| t.name == current);
                let selected_index = selected.map(|i| IndexPath::default().row(i));
                let theme_select =
                    cx.new(|cx| SelectState::new(theme_names, selected_index, window, cx));

                // 日志级别下拉
                let log_levels: Vec<SharedString> =
                    ["off", "error", "warn", "info", "debug", "trace"]
                        .iter()
                        .map(|s| SharedString::from(*s))
                        .collect();
                let current_level = config::load_settings().log_level.to_lowercase();
                let selected = log_levels
                    .iter()
                    .position(|s| s.to_string() == current_level);
                let selected_index = selected.map(|i| IndexPath::default().row(i));
                let log_level_select =
                    cx.new(|cx| SelectState::new(log_levels, selected_index, window, cx));

                let app_view = cx.new(|cx| {
                    let mut v = AppView::new(
                        init,
                        name_input,
                        content_input,
                        theme_select.clone(),
                        log_level_select.clone(),
                        window,
                        cx,
                    );
                    v.detect_frpc_version(cx);
                    AppView::start_health_monitor(cx);
                    v
                });

                cx.new(|cx| Root::new(app_view, window, cx))
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
