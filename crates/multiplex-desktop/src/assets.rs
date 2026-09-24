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
    icon!("status/diamond"),
    icon!("status/dashed-circle"),
    icon!("status/filled-circle"),
    icon!("status/hollow-circle"),
    icon!("status/hollow-diamond"),
    icon!("status/hollow-square"),
    icon!("status/octagon"),
    icon!("status/ring-spinner"),
    icon!("arrow-down"),
    icon!("arrow-left"),
    icon!("arrow-right"),
    icon!("arrow-up"),
    icon!("book-open"),
    icon!("calendar"),
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
    icon!("folder"),
    icon!("folder-open"),
    icon!("globe"),
    icon!("grid"),
    icon!("info"),
    icon!("key"),
    icon!("keyboard"),
    icon!("magnifying-glass"),
    icon!("maximize"),
    icon!("minimize"),
    icon!("notebook"),
    icon!("palette"),
    icon!("panel-bottom"),
    icon!("panel-collapse-right"),
    icon!("pencil"),
    icon!("panel-right"),
    icon!("plus"),
    icon!("redo"),
    icon!("redo-2"),
    icon!("search"),
    icon!("settings"),
    icon!("tag"),
    icon!("shield-check"),
    icon!("square-terminal"),
    icon!("terminal-window"),
    icon!("trash"),
    icon!("triangle-alert"),
    icon!("user"),
    icon!("vault"),
    icon!("x"),
    icon!("a-large-small"),
    icon!("bell"),
    icon!("bot"),
    icon!("building-2"),
    icon!("circle-x"),
    icon!("external-link"),
    icon!("eye"),
    icon!("file"),
    icon!("folder-closed"),
    icon!("frame"),
    icon!("github"),
    icon!("house"),
    icon!("inbox"),
    icon!("inspector"),
    icon!("layout-dashboard"),
    icon!("loader-circle"),
    icon!("magnet"),
    icon!("map"),
    icon!("menu"),
    icon!("minus"),
    icon!("panel-left"),
    icon!("panel-left-close"),
    icon!("panel-left-open"),
    icon!("panel-right-close"),
    icon!("panel-right-open"),
    icon!("replace"),
    icon!("resize-corner"),
    icon!("sort-ascending"),
    icon!("sort-descending"),
    icon!("star"),
    icon!("undo"),
    icon!("undo-2"),
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// gpui-component's file name for an `IconName` variant, where it is not the kebab case of
    /// the variant's name.
    fn component_file(variant: &str) -> String {
        match variant {
            "GitHub" => "github".to_owned(),
            _ => {
                let mut file = String::new();
                for (index, character) in variant.chars().enumerate() {
                    if index > 0 && (character.is_ascii_uppercase() || character.is_ascii_digit()) {
                        file.push('-');
                    }
                    file.push(character.to_ascii_lowercase());
                }
                file
            }
        }
    }

    fn sources(directory: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(std::fs::read_to_string(path).unwrap());
            }
        }
    }

    /// An icon the app names but does not embed draws as nothing, silently.
    #[test]
    fn every_icon_the_app_names_is_embedded() {
        let mut files = Vec::new();
        sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut missing = Vec::new();
        for text in &files {
            for (index, _) in text.match_indices("IconName::") {
                let variant: String = text[index + "IconName::".len()..]
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect();
                if variant.is_empty() || !variant.starts_with(|c: char| c.is_ascii_uppercase()) {
                    continue;
                }
                let path = format!("icons/{}.svg", component_file(&variant));
                if Assets.load(&path).unwrap().is_none() {
                    missing.push(format!("IconName::{variant} ({path})"));
                }
            }
            for (index, _) in text.match_indices("\"icons/") {
                let literal: String = text[index + 1..]
                    .chars()
                    .take_while(|character| *character != '"')
                    .collect();
                if literal.ends_with(".svg")
                    && !literal.contains('{')
                    && Assets.load(&literal).unwrap().is_none()
                {
                    missing.push(literal);
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "icons named but not embedded: {missing:?}"
        );
    }
}
