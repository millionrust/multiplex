use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub struct Assets;

const ASSET_PATHS: &[&str] = &[
    "icons/folder-open.svg",
    "icons/key.svg",
    "icons/magnifying-glass.svg",
    "icons/notebook.svg",
    "icons/plus.svg",
    "icons/shield-check.svg",
    "icons/terminal-window.svg",
    "icons/trash.svg",
    "icons/x.svg",
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes = match path {
            "icons/folder-open.svg" => {
                Some(include_bytes!("../assets/icons/folder-open.svg").as_slice())
            }
            "icons/key.svg" => Some(include_bytes!("../assets/icons/key.svg").as_slice()),
            "icons/magnifying-glass.svg" => {
                Some(include_bytes!("../assets/icons/magnifying-glass.svg").as_slice())
            }
            "icons/notebook.svg" => Some(include_bytes!("../assets/icons/notebook.svg").as_slice()),
            "icons/plus.svg" => Some(include_bytes!("../assets/icons/plus.svg").as_slice()),
            "icons/shield-check.svg" => {
                Some(include_bytes!("../assets/icons/shield-check.svg").as_slice())
            }
            "icons/terminal-window.svg" => {
                Some(include_bytes!("../assets/icons/terminal-window.svg").as_slice())
            }
            "icons/trash.svg" => Some(include_bytes!("../assets/icons/trash.svg").as_slice()),
            "icons/x.svg" => Some(include_bytes!("../assets/icons/x.svg").as_slice()),
            _ => None,
        };

        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ASSET_PATHS
            .iter()
            .filter(|entry| path.is_empty() || entry.starts_with(path))
            .map(|entry| (*entry).into())
            .collect())
    }
}
