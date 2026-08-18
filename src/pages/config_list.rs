//! 配置列表页面：卡片网格展示所有配置，支持分页（每页最多8个配置 + 1个添加卡 = 9个，3行×3列）

use crate::config::FrpcConfigMeta;
use crate::icons::AppIcon;
use crate::message;
use crate::message::MessageLevel;
use gpui::prelude::*;
use gpui::{div, px, ClipboardItem, FontWeight};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants};
use gpui_component::dialog::DialogButtonProps;
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme, Disableable, Icon, Sizable, Size, WindowExt};

use crate::app::AppView;

/// 每页最多显示的配置数量（不含添加卡）
const PAGE_SIZE: usize = 8;

pub fn render(view: &mut AppView, cx: &mut Context<AppView>) -> gpui::AnyElement {
    let configs = view.configs.clone();
    let running_keys: Vec<String> = view.running.keys().cloned().collect();
    let current_page = view.config_page;

    let total_pages = ((configs.len() + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page_configs: Vec<_> = configs
        .into_iter()
        .skip(current_page * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();

    let mut content = div()
        .flex_1()
        .h_full()
        .bg(cx.theme().background)
        .flex()
        .flex_col();

    // 顶部标题栏
    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(24.0))
        .py(px(16.0))
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(cx.theme().foreground)
                .child("配置列表"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_x(px(8.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("共 {} 个配置", view.configs.len())),
                )
                .child(
                    Button::new("btn-add-config")
                        .primary()
                        .icon(AppIcon::Plus)
                        .label("添加配置")
                        .on_click(cx.listener(|view, _event, window, cx| {
                            view.open_add_config(window, cx);
                        })),
                ),
        );

    // 配置卡片网格
    let mut grid = div()
        .flex()
        .flex_wrap()
        .justify_between()
        .gap_y(px(16.0))
        .px(px(24.0))
        .pb(px(16.0));

    for config_item in &page_configs {
        let is_running = running_keys.contains(&config_item.name);
        let card = render_config_card(config_item, is_running, cx);
        grid = grid.child(card);
    }

    // 添加配置的虚线卡片（始终显示在最后）
    let add_card = div()
        .w(px(240.0))
        .h(px(180.0))
        .rounded(px(8.0))
        .border_2()
        .border_dashed()
        .border_color(cx.theme().border)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_y(px(8.0))
        .cursor_pointer()
        .hover(|s| s.border_color(cx.theme().primary))
        .id("add-card")
        .on_click(cx.listener(|view, _event, window, cx| {
            view.open_add_config(window, cx);
        }))
        .child(
            div()
                .text_xl()
                .text_color(cx.theme().muted_foreground)
                .child("+"),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("添加配置"),
        );

    grid = grid.child(add_card);

    // 补齐最后一行的占位，确保 justify_between 对齐
    let total_items = page_configs.len() + 1; // 配置卡片 + 添加卡片
    let remainder = total_items % 3;
    if remainder != 0 {
        for _ in 0..(3 - remainder) {
            grid = grid.child(div().w(px(240.0)).h(px(180.0)));
        }
    }

    // 可滚动的内容区域（header 固定，grid + 分页可滚动）
    let mut scrollable = div()
        .flex_1()
        .id("config-list-scroll")
        .overflow_y_scroll()
        .child(grid);

    // 分页控件
    if total_pages > 1 {
        let has_prev = current_page > 0;
        let has_next = current_page + 1 < total_pages;

        let pagination = div()
            .flex()
            .items_center()
            .justify_center()
            .gap_x(px(12.0))
            .px(px(24.0))
            .pb(px(16.0))
            .child(
                Button::new("btn-prev-page")
                    .label("上一页")
                    .when(!has_prev, |b| b.disabled(true))
                    .on_click(cx.listener(|view, _event, _window, cx| {
                        if view.config_page > 0 {
                            view.config_page -= 1;
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{}/{}", current_page + 1, total_pages)),
            )
            .child(
                Button::new("btn-next-page")
                    .label("下一页")
                    .when(!has_next, |b| b.disabled(true))
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        let total = ((view.configs.len() + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
                        if view.config_page + 1 < total {
                            view.config_page += 1;
                            cx.notify();
                        }
                    })),
            );

        scrollable = scrollable.child(pagination);
    }

    // 处理中指示器
    if view.is_processing {
        scrollable = scrollable.child(
            div()
                .mx(px(24.0))
                .mb(px(16.0))
                .flex()
                .items_center()
                .gap_x(px(8.0))
                .child(Spinner::new())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("正在处理..."),
                ),
        );
    }

    content = content.child(header).child(scrollable);

    div()
        .relative()
        .flex_1()
        .h_full()
        .child(content)
        .when_some(view.status_message.clone(), |el, msg| {
            el.child(message::toast(&view.status_level, &msg, cx.theme()))
        })
        .into_any_element()
}

fn render_config_card(
    meta: &FrpcConfigMeta,
    is_running: bool,
    cx: &mut Context<AppView>,
) -> gpui::AnyElement {
    let name = meta.name.clone();
    let name_for_action = name.clone();
    let name_for_edit = name.clone();
    let name_for_delete = name.clone();

    let status_text = if is_running { "运行中" } else { "未启动" };
    let status_color = if is_running {
        cx.theme().success
    } else {
        cx.theme().danger
    };

    let server_addr_display = if meta.server_addr.is_empty() {
        "未配置服务器".to_string()
    } else {
        meta.server_addr.clone()
    };
    let server_addr_raw = meta.server_addr.clone();
    let can_copy = !server_addr_raw.is_empty();

    // 构建代理显示数据
    let mut proxy_items: Vec<(String, String, String)> = Vec::new();
    for proxy in meta.proxies.iter().take(2) {
        let ptype = if proxy.proxy_type.is_empty() {
            "tcp".to_string()
        } else {
            proxy.proxy_type.clone()
        };
        let local = proxy
            .local_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "?".to_string());
        let remote = proxy
            .remote_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "?".to_string());
        proxy_items.push((ptype, local, remote));
    }
    let has_more = meta.proxies.len() > 2;
    let proxy_empty = meta.proxies.is_empty();

    div()
        .w(px(240.0))
        .h(px(180.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar)
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap_y(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(name.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x(px(6.0))
                        .child(div().text_xs().text_color(status_color).child(status_text))
                        .child(
                            div()
                                .w(px(8.0))
                                .h(px(8.0))
                                .rounded(px(4.0))
                                .bg(status_color),
                        ),
                ),
        )
        // 分割线
        .child(div().w_full().h(px(1.0)).bg(cx.theme().border))
        .child({
            let sa = server_addr_raw.clone();
            div().w_full().flex().justify_center().child(
                Button::new(format!("btn-copy-sa-{}", name))
                    .ghost()
                    .with_size(Size::XSmall)
                    .compact()
                    .tooltip("点击复制")
                    .label(server_addr_display)
                    .when(can_copy, |b| {
                        b.on_click(cx.listener(move |view, _event, _window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(sa.clone()));
                            view.set_status_message(
                                format!("已复制：{}", sa),
                                MessageLevel::Success,
                                cx,
                            );
                        }))
                    }),
            )
        })
        // 分割线
        .child(div().w_full().h(px(1.0)).bg(cx.theme().border))
        .child({
            let mut section = div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_y(px(2.0));

            if proxy_empty {
                section = section.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("无代理配置"),
                );
            } else {
                for (ptype, local, remote) in &proxy_items {
                    let ptype = ptype.clone();
                    let local = local.clone();
                    let remote = remote.clone();
                    let sa = server_addr_raw.clone();
                    let remote_for_copy = remote.clone();
                    let btn_id = format!("btn-copy-remote-{}-{}-{}", name, ptype, local);
                    section = section.child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_x(px(4.0))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{} {}", ptype, local)),
                                    )
                                    .child(
                                        Icon::new(AppIcon::ArrowRight)
                                            .text_color(cx.theme().muted_foreground)
                                            .with_size(Size::XSmall),
                                    ),
                            )
                            .child(
                                Button::new(btn_id)
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .compact()
                                    .tooltip("点击复制")
                                    .label(remote)
                                    .when(can_copy, |b| {
                                        b.on_click(cx.listener(move |view, _event, _window, cx| {
                                            let copy_text = format!("{}:{}", sa, remote_for_copy);
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy_text.clone(),
                                            ));
                                            view.set_status_message(
                                                format!("已复制：{}", copy_text),
                                                MessageLevel::Success,
                                                cx,
                                            );
                                        }))
                                    }),
                            ),
                    );
                }
                if has_more {
                    section = section.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("..."),
                    );
                }
            }

            section
        })
        // 分割线
        .child(div().w_full().h(px(1.0)).bg(cx.theme().border))
        .child({
            let border_color = cx.theme().border;
            let divider = || div().w(px(1.0)).h(px(16.0)).bg(border_color);
            let mut actions = div().flex().items_center().w_full();

            // 停止 / 启动
            actions = actions.child(div().flex_1().flex().justify_center().child(if is_running {
                Button::new(format!("btn-stop-{}", name))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .foreground(cx.theme().danger)
                            .hover(cx.theme().danger.opacity(0.15))
                            .active(cx.theme().danger.opacity(0.3)),
                    )
                    .tooltip("停止")
                    .icon(AppIcon::Square)
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        view.stop_config(&name_for_action, cx);
                    }))
                    .into_any_element()
            } else {
                Button::new(format!("btn-start-{}", name))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .foreground(cx.theme().success)
                            .hover(cx.theme().success.opacity(0.15))
                            .active(cx.theme().success.opacity(0.3)),
                    )
                    .tooltip("启动")
                    .icon(AppIcon::Play)
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        view.start_config(&name_for_action, cx);
                    }))
                    .into_any_element()
            }));

            // 重启（仅运行中的配置显示）
            if is_running {
                let name_for_restart = name.clone();
                actions = actions.child(divider()).child(
                    div().flex_1().flex().justify_center().child(
                        Button::new(format!("btn-restart-{}", name))
                            .ghost()
                            .tooltip("重启")
                            .icon(AppIcon::RotateCcw)
                            .on_click(cx.listener(move |view, _event, _window, cx| {
                                view.restart_config(&name_for_restart, cx);
                            })),
                    ),
                );
            }

            actions = actions
                .child(divider())
                .child(
                    div().flex_1().flex().justify_center().child(
                        Button::new(format!("btn-edit-{}", name))
                            .ghost()
                            .tooltip("编辑")
                            .icon(AppIcon::SquarePen)
                            .on_click(cx.listener(move |view, _event, window, cx| {
                                view.open_edit_config(&name_for_edit, window, cx);
                            })),
                    ),
                )
                .child(divider())
                .child(
                    div().flex_1().flex().justify_center().child(
                        Button::new(format!("btn-delete-{}", name))
                            .ghost()
                            .tooltip("删除")
                            .icon(AppIcon::Trash)
                            .on_click(cx.listener(move |_view, _event, window, cx| {
                                // 弹窗确认后再删除
                                let entity = cx.entity();
                                let nc = name_for_delete.clone();
                                window.open_alert_dialog(cx, move |alert, _window, _cx| {
                                    let entity = entity.clone();
                                    let nc = nc.clone();
                                    alert
                                        .title("删除配置？")
                                        .description(format!(
                                            "确定要删除配置 '{}' 吗？此操作不可恢复。",
                                            nc
                                        ))
                                        .button_props(
                                            DialogButtonProps::default()
                                                .ok_text("删除")
                                                .ok_variant(ButtonVariant::Danger)
                                                .cancel_text("取消")
                                                .show_cancel(true),
                                        )
                                        .on_ok(move |_event, _window, cx| {
                                            entity
                                                .update(cx, |view, cx| view.delete_config(&nc, cx));
                                            true
                                        })
                                });
                            })),
                    ),
                );

            actions
        })
        .into_any_element()
}
