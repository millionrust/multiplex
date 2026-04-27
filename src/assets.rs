use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub struct Assets;

macro_rules! icon {
    ($path:literal) => {
        (
            concat!("icons/", $path, ".svg"),
            include_bytes!(concat!("../assets/icons/", $path, ".svg")).as_slice(),
        )
    };
}

const ICONS: &[(&str, &[u8])] = &[
    icon!("arrow-down"),
    icon!("arrow-left"),
    icon!("arrow-right"),
    icon!("arrow-up"),
    icon!("book-open"),
    icon!("check"),
    icon!("chevron-down"),
    icon!("chevron-left"),
    icon!("chevron-right"),
    icon!("chevron-up"),
    icon!("circle-check"),
    icon!("close"),
    icon!("copy"),
    icon!("delete"),
    icon!("ellipsis"),
    icon!("folder-open"),
    icon!("globe"),
    icon!("info"),
    icon!("key"),
    icon!("magnifying-glass"),
    icon!("maximize"),
    icon!("minimize"),
    icon!("notebook"),
    icon!("panel-bottom"),
    icon!("panel-right"),
    icon!("plus"),
    icon!("redo"),
    icon!("search"),
    icon!("settings"),
    icon!("shield-check"),
    icon!("square-terminal"),
    icon!("terminal-window"),
    icon!("trash"),
    icon!("triangle-alert"),
    icon!("user"),
    icon!("vault"),
    icon!("x"),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        for (icon_path, bytes) in ICONS {
            if *icon_path == path {
                return Ok(Some(Cow::Borrowed(bytes)));
            }
        }

        Ok(None)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(icon_path, _)| path.is_empty() || icon_path.starts_with(path))
            .map(|(icon_path, _)| (*icon_path).into())
            .collect())
    }
}
