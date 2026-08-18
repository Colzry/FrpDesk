//! 自定义图标定义

use gpui::SharedString;
use gpui_component::IconNamed;
use std::borrow::Cow;

/// 自定义图标枚举
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AppIcon {
    ArrowLeft,
    CircleCheckGreen,
    CircleXRed,
    FileSliders,
    FileSlidersBlack,
    FileSlidersLight,
    FileSlidersWhite,
    InfoBlue,
    InfoYellow,
    Play,
    Plus,
    RotateCcw,
    Settings,
    SettingsBlack,
    SettingsLight,
    SettingsWhite,
    Square,
    SquarePen,
    Trash,
}

impl IconNamed for AppIcon {
    fn path(self) -> SharedString {
        match self {
            Self::ArrowLeft => "icons/arrow-left.svg".into(),
            Self::CircleCheckGreen => "icons/circle-check-green.svg".into(),
            Self::CircleXRed => "icons/circle-x-red.svg".into(),
            Self::FileSliders => "icons/file-sliders.svg".into(),
            Self::FileSlidersBlack => "icons/file-sliders-black.svg".into(),
            Self::FileSlidersLight => "icons/file-sliders-light.svg".into(),
            Self::FileSlidersWhite => "icons/file-sliders-white.svg".into(),
            Self::InfoBlue => "icons/info-blue.svg".into(),
            Self::InfoYellow => "icons/info-yellow.svg".into(),
            Self::Play => "icons/play.svg".into(),
            Self::Plus => "icons/plus.svg".into(),
            Self::RotateCcw => "icons/rotate-ccw.svg".into(),
            Self::Settings => "icons/settings.svg".into(),
            Self::SettingsBlack => "icons/settings-black.svg".into(),
            Self::SettingsLight => "icons/settings-light.svg".into(),
            Self::SettingsWhite => "icons/settings-white.svg".into(),
            Self::Square => "icons/square.svg".into(),
            Self::SquarePen => "icons/square-pen.svg".into(),
            Self::Trash => "icons/trash.svg".into(),
        }
    }
}

/// 自定义资源源，嵌入所有 SVG 图标
pub struct AppAssets;

impl gpui::AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let data: Option<Cow<'static, [u8]>> = match path {
            "icons/arrow-left.svg" => Some(Cow::Borrowed(
                include_bytes!("../icons/arrow-left.svg") as &[u8],
            )),
            "icons/circle-check-green.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/circle-check-green.svg"
            ) as &[u8])),
            "icons/circle-x-red.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/circle-x-red.svg"
            ) as &[u8])),
            "icons/file-sliders.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/file-sliders.svg"
            ) as &[u8])),
            "icons/file-sliders-black.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/file-sliders-black.svg"
            ) as &[u8])),
            "icons/file-sliders-light.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/file-sliders-light.svg"
            ) as &[u8])),
            "icons/file-sliders-white.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/file-sliders-white.svg"
            ) as &[u8])),
            "icons/info-blue.svg" => Some(Cow::Borrowed(
                include_bytes!("../icons/info-blue.svg") as &[u8]
            )),
            "icons/info-yellow.svg" => Some(Cow::Borrowed(
                include_bytes!("../icons/info-yellow.svg") as &[u8],
            )),
            "icons/play.svg" => Some(Cow::Borrowed(include_bytes!("../icons/play.svg") as &[u8])),
            "icons/plus.svg" => Some(Cow::Borrowed(include_bytes!("../icons/plus.svg") as &[u8])),
            "icons/rotate-ccw.svg" => Some(Cow::Borrowed(
                include_bytes!("../icons/rotate-ccw.svg") as &[u8],
            )),
            "icons/settings.svg" => Some(Cow::Borrowed(
                include_bytes!("../icons/settings.svg") as &[u8]
            )),
            "icons/settings-black.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/settings-black.svg"
            ) as &[u8])),
            "icons/settings-light.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/settings-light.svg"
            ) as &[u8])),
            "icons/settings-white.svg" => Some(Cow::Borrowed(include_bytes!(
                "../icons/settings-white.svg"
            ) as &[u8])),
            "icons/square.svg" => {
                Some(Cow::Borrowed(include_bytes!("../icons/square.svg") as &[u8]))
            }
            "icons/square-pen.svg" => Some(Cow::Borrowed(
                include_bytes!("../icons/square-pen.svg") as &[u8],
            )),
            "icons/trash.svg" => Some(Cow::Borrowed(include_bytes!("../icons/trash.svg") as &[u8])),
            _ => None,
        };
        Ok(data)
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec![])
    }
}
