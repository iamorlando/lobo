use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn collect(root: &Path, path: &Path, entries: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", path.display());
    for item in fs::read_dir(path).expect("bundled terminal skill must exist") {
        let path = item.unwrap().path();
        if path.is_dir() {
            collect(root, &path, entries);
        } else if path.is_file() {
            entries.push((
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                path,
            ));
        }
    }
}
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../../skills/lobo-terminal");
    let mut entries = Vec::new();
    collect(&root, &root, &mut entries);
    entries.sort();
    let mut code = String::from("pub const SKILL_FILES: &[(&str, &str)] = &[\n");
    for (name, path) in entries {
        code.push_str(&format!(
            "({name:?}, include_str!({:?})),\n",
            path.canonicalize().unwrap()
        ));
    }
    code.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("skill_bundle.rs"),
        code,
    )
    .unwrap();
}
