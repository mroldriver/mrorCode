//! 构建期把 src/themes/*.json 打包成 BUNDLED（零运行时文件依赖，无 include_dir crate）。

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from("src/themes");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("themes_bundled.rs");

    let mut entries: Vec<(String, String)> = fs::read_dir(&dir)
        .expect("read themes dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap();
            let content = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"));
            (name, content)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut code = String::from(
        "/// Vendored opencode themes (MIT, see licenses/OPENCODE-LICENSE).\n\
         pub static BUNDLED: &[(&str, &str)] = &[\n",
    );
    for (name, content) in &entries {
        code.push_str(&format!("    ({name:?}, {content:?}),\n"));
    }
    code.push_str("];\n");

    fs::write(&out, code).unwrap();
    println!("cargo:rerun-if-changed=src/themes");
}
