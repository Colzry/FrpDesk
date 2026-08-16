//! 配置编辑器页面：名称输入 + 多行内容输入 + 保存/返回

use crate::icons::AppIcon;
use crate::message;
use gpui::prelude::*;
use gpui::{div, px, FontWeight};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Editor, Input};
use gpui_component::switch::Switch;
use gpui_component::ActiveTheme;

use crate::app::{AppView, Page};

pub fn render(view: &mut AppView, cx: &mut Context<AppView>) -> gpui::AnyElement {
    let is_edit = matches!(
        view.page,
        Page::ConfigEditor {
            original_name: Some(_)
        }
    );
    let title = if is_edit {
        "编辑配置"
    } else {
        "添加配置"
    };

    div()
        .flex_1()
        .h_full()
        .bg(cx.theme().background)
        .flex()
        .flex_col()
        .relative()
        // 顶部标题栏
        .child(
            div()
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
                        .child(title),
                )
                .child(
                    Button::new("btn-back")
                        .icon(AppIcon::ArrowLeft)
                        .label("返回")
                        .on_click(cx.listener(|view, _event, _window, cx| {
                            view.switch_page(Page::ConfigList, cx);
                            view.reload_configs(cx);
                        })),
                ),
        )
        // 配置名称输入
        .child(
            div()
                .mx(px(24.0))
                .mb(px(12.0))
                .flex()
                .flex_col()
                .gap_y(px(6.0))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child("配置名称"),
                )
                .child(Input::new(&view.name_input)),
        )
        // 配置内容编辑（多行文本域）
        .child(
            div()
                .mx(px(24.0))
                .mb(px(12.0))
                .flex()
                .flex_col()
                .gap_y(px(6.0))
                .flex_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child("配置内容 (TOML)"),
                )
                .child(Editor::new(&view.content_input).h_full()),
        )
        // 自启动开关
        .child(
            div()
                .mx(px(24.0))
                .mb(px(12.0))
                .flex()
                .items_center()
                .gap_x(px(8.0))
                .child(
                    Switch::new("switch-auto-start")
                        .checked(view.edit_auto_start)
                        .on_click(cx.listener(|view, _checked: &bool, _window, cx| {
                            view.edit_auto_start = !view.edit_auto_start;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child("开机自启动"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("该功能需要设置中注册服务后才生效"),
                ),
        )
        // 底部按钮栏
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .px(px(24.0))
                .py(px(16.0))
                .gap_x(px(12.0))
                .child(
                    Button::new("btn-cancel")
                        .label("取消")
                        .on_click(cx.listener(|view, _event, _window, cx| {
                            view.switch_page(Page::ConfigList, cx);
                            view.reload_configs(cx);
                        })),
                )
                .child(
                    Button::new("btn-save")
                        .primary()
                        .label("保存")
                        .on_click(cx.listener(|view, _event, _window, cx| {
                            view.save_config(cx);
                        })),
                ),
        )
        .when_some(view.status_message.clone(), |el, msg| {
            el.child(message::toast(&view.status_level, &msg, cx.theme()))
        })
        .into_any_element()
}
