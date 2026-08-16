//! 设置页面：frpc 版本信息、下载、服务注册/注销、日志

use crate::message;
use gpui::prelude::*;
use gpui::{div, px, FontWeight};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::scroll::ScrollableElement;
use gpui_component::select::Select;
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme, Disableable, Sizable, Size};

use crate::app::AppView;
use crate::download;

/// 分割线
fn separator(theme: &gpui_component::ThemeColor) -> gpui::Div {
    div().w_full().h(px(1.0)).bg(theme.border)
}

pub fn render(view: &mut AppView, cx: &mut Context<AppView>) -> gpui::AnyElement {
    let has_frpc = download::has_frpc_executable(
        &std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from(".")),
    );

    let inner = div()
        .flex_1()
        .h_full()
        .bg(cx.theme().background)
        .flex()
        .flex_col()
        .overflow_y_scrollbar()
        // 标题
        .child(
            div().px(px(24.0)).py(px(16.0)).child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(cx.theme().foreground)
                    .child("设置"),
            ),
        )
        .child(div().mx(px(24.0)).child(separator(cx.theme())))
        // ========== frpc 程序 ==========
        .child(
            div()
                .mx(px(24.0))
                .py(px(16.0))
                .flex()
                .flex_col()
                .gap_y(px(12.0))
                // 标题行：标题 + 状态
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("frpc 程序"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(if has_frpc {
                                    cx.theme().success
                                } else {
                                    cx.theme().danger
                                })
                                .child(if has_frpc { "已安装" } else { "未安装" }),
                        ),
                )
                // 版本 + 更新按钮
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "版本：{}",
                                    view.frpc_version
                                        .clone()
                                        .unwrap_or_else(|| "未知".to_string())
                                )),
                        )
                        .child({
                            let btn_label = if has_frpc { "更新" } else { "下载" };
                            Button::new("btn-download")
                                .with_size(Size::Small)
                                .primary()
                                .label(btn_label)
                                .when(view.is_checking_update || view.is_downloading, |b| {
                                    b.disabled(true)
                                })
                                .on_click(cx.listener(|view, _event, window, cx| {
                                    // 先检查版本：有更新且 frpc 正在运行时，才会在下载前弹窗确认
                                    view.start_download(window, cx);
                                }))
                        })
                        .when(view.is_checking_update, |el| {
                            el.child(Spinner::new()).child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("检查版本更新中..."),
                            )
                        })
                        .when(view.is_downloading, |el| {
                            el.child(Spinner::new()).child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{}%", view.download_percent)),
                            )
                        }),
                ),
        )
        .child(div().mx(px(24.0)).child(separator(cx.theme())))
        // ========== Windows 服务 ==========
        .child(
            div()
                .mx(px(24.0))
                .py(px(16.0))
                .flex()
                .flex_col()
                .gap_y(px(12.0))
                // 标题行：标题 + 状态 + 注册/注销按钮
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("Windows 服务"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_x(px(6.0))
                                .child(div().w(px(8.0)).h(px(8.0)).rounded(px(4.0)).bg(
                                    if view.service_registered {
                                        cx.theme().success
                                    } else {
                                        cx.theme().danger
                                    },
                                ))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(if view.service_registered {
                                            cx.theme().success
                                        } else {
                                            cx.theme().danger
                                        })
                                        .child(if view.service_registered {
                                            "已注册"
                                        } else {
                                            "未注册"
                                        }),
                                ),
                        )
                        .child(if view.service_registered {
                            Button::new("btn-uninstall")
                                .with_size(Size::Small)
                                .danger()
                                .label("注销服务")
                                .on_click(cx.listener(|view, _event, _window, cx| {
                                    view.uninstall_service(cx);
                                }))
                                .into_any_element()
                        } else {
                            Button::new("btn-install")
                                .with_size(Size::Small)
                                .primary()
                                .label("注册服务")
                                .on_click(cx.listener(|view, _event, _window, cx| {
                                    view.install_service(cx);
                                }))
                                .into_any_element()
                        })
                        .when(view.is_processing, |el| {
                            el.child(Spinner::new()).child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("处理中..."),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("服务注册后，开启自启动的配置会开机自启。"),
                ),
        )
        .child(div().mx(px(24.0)).child(separator(cx.theme())))
        // ========== 主题设置 ==========
        .child(
            div()
                .mx(px(24.0))
                .py(px(16.0))
                .flex()
                .flex_col()
                .gap_y(px(12.0))
                // 标题行：标题 + 下拉列表
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("主题设置"),
                        )
                        .child(div().w(px(200.0)).child(Select::new(&view.theme_select))),
                ),
        )
        .child(div().mx(px(24.0)).child(separator(cx.theme())))
        // ========== 进程守护 ==========
        .child(
            div()
                .mx(px(24.0))
                .py(px(16.0))
                .flex()
                .flex_col()
                .gap_y(px(12.0))
                // 标题行：标题 + 状态 + 开关
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("进程守护"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(if view.process_guard {
                                    cx.theme().success
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(if view.process_guard {
                                    "已开启"
                                } else {
                                    "未开启"
                                }),
                        )
                        .child({
                            // 小开关
                            let enabled = view.service_registered;
                            div()
                                .id("switch-process-guard")
                                .w(px(36.0))
                                .h(px(20.0))
                                .rounded(px(10.0))
                                .bg(if view.process_guard {
                                    cx.theme().primary
                                } else {
                                    cx.theme().border
                                })
                                .when(!enabled, |el| el.opacity(0.4))
                                .when(enabled, |el| el.cursor_pointer())
                                .when(!enabled, |el| el.cursor_not_allowed())
                                .flex()
                                .items_center()
                                .px(px(2.0))
                                .child(
                                    div()
                                        .w(px(16.0))
                                        .h(px(16.0))
                                        .rounded_full()
                                        .bg(gpui::rgb(0xffffff))
                                        .when(view.process_guard && enabled, |el| el.ml_auto()),
                                )
                                .when(enabled, |el| {
                                    el.on_click(cx.listener(|view, _event, _window, cx| {
                                        view.toggle_process_guard(cx);
                                    }))
                                })
                        }),
                )
                // 说明
                .child(
                    div()
                        .text_xs()
                        .text_color(if view.service_registered {
                            cx.theme().muted_foreground
                        } else {
                            cx.theme().danger
                        })
                        .child(if view.service_registered {
                            "开启后，设置开机自启动的配置会立即启动；frpc 进程异常退出时会自动重启。"
                        } else {
                            "请先注册服务后再开启进程守护。"
                        }),
                ),
        )
        .child(div().mx(px(24.0)).child(separator(cx.theme())))
        // ========== 日志 ==========
        .child(
            div()
                .mx(px(24.0))
                .py(px(16.0))
                .flex()
                .flex_col()
                .gap_y(px(12.0))
                // 标题行：标题 + 按钮
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x(px(12.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("日志"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("打印级别"),
                        )
                        .child(
                            div()
                                .w(px(140.0))
                                .child(Select::new(&view.log_level_select)),
                        )
                        .child(
                            Button::new("btn-open-logs")
                                .with_size(Size::Small)
                                .label("打开日志目录")
                                .on_click(cx.listener(|_view, _event, _window, _cx| {
                                    let logs_dir = std::env::current_exe()
                                        .ok()
                                        .and_then(|p| p.parent().map(|p| p.join("logs")));
                                    if let Some(dir) = logs_dir {
                                        let _ =
                                            std::process::Command::new("explorer").arg(dir).spawn();
                                    }
                                })),
                        ),
                ),
        )
        .child(div().mx(px(24.0)).child(separator(cx.theme())));

    div()
        .relative()
        .flex_1()
        .h_full()
        .child(inner)
        .when_some(view.status_message.clone(), |el, msg| {
            el.child(message::toast(&view.status_level, &msg, cx.theme()))
        })
        .into_any_element()
}
