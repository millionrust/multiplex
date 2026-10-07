use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn walk(root: &Path, path: &Path, out: &mut String) {
    let mut entries = fs::read_dir(path)
        .expect("embedded browser assets")
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for file in entries {
        if file.is_dir() {
            walk(root, &file, out);
        } else {
            let name = format!(
                "/{}",
                file.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            );
            let mime = match file.extension().and_then(|s| s.to_str()).unwrap_or("") {
                "html" => "text/html; charset=utf-8",
                "js" => "text/javascript; charset=utf-8",
                "css" => "text/css; charset=utf-8",
                "woff" => "font/woff",
                "woff2" => "font/woff2",
                "json" => "application/json",
                "svg" => "image/svg+xml",
                _ => "application/octet-stream",
            };
            out.push_str(&format!(
                "{name:?} => Some((include_bytes!({:?}), {mime:?})),\n",
                file.canonicalize().unwrap()
            ));
        }
    }
}
fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/web-terminal");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut code =
        "pub fn embedded(path: &str) -> Option<(&'static [u8], &'static str)> { match path {\n"
            .to_string();
    walk(&root, &root, &mut code);
    code.push_str("_ => None, } }\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("web_assets.rs"),
        code,
    )
    .unwrap();
}
