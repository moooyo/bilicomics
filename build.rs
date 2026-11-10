use std::{collections::BTreeMap, env, fs, path::PathBuf};

#[path = "src/source/native_build.rs"]
mod native_build;

fn main() {
    native_build::build();
    println!("cargo:rerun-if-changed=assets/i18n");
    println!("cargo:rerun-if-changed=assets/design-tokens.json");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let mut paths: Vec<_> = fs::read_dir("assets/i18n")
        .expect("Read translation directory")
        .map(|entry| entry.expect("Read translation entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    paths.sort();
    let mut labels = BTreeMap::<String, String>::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let entries: BTreeMap<String, String> = serde_json::from_slice(
            &fs::read(&path).expect("Read interface translations"),
        )
        .expect("Parse interface translations");
        for (key, value) in entries {
            if let Some(previous) = labels.insert(key.clone(), value.clone()) {
                assert_eq!(previous, value, "Conflicting translation: {key}");
            }
        }
    }
    write_if_changed(&output.join("translations.json"), &serde_json::to_vec(&labels).expect("Serialize translations"))
        .expect("Write runtime translations");
    let mut translations = String::from("export global Strings {\n");
    for (key, value) in labels {
        translations.push_str(&format!(
            "    out property <string> {}: {};\n",
            key.replace('_', "-"),
            serde_json::to_string(&value).expect("Escape interface text")
        ));
    }
    translations.push_str("}\n");
    write_if_changed(&output.join("strings.slint"), translations.as_bytes()).expect("Write generated translations");
    let mut demo_assets = String::from("pub static DEMO_ASSETS: &[(&str, &[u8])] = &[\n");
    let demo_root = PathBuf::from("assets/demo");
    println!("cargo:rerun-if-changed=assets/demo");
    let mut asset_paths = Vec::new();
    collect_files(&demo_root, &mut asset_paths);
    asset_paths.sort();
    for path in asset_paths {
        let name = path.strip_prefix(&demo_root).expect("Relative demo asset").to_string_lossy().replace('\\', "/");
        let absolute = fs::canonicalize(&path).expect("Resolve demo asset");
        demo_assets.push_str(&format!("({}, include_bytes!({})),\n",
            serde_json::to_string(&name).expect("Escape asset name"),
            serde_json::to_string(&absolute.to_string_lossy()).expect("Escape asset path")));
        println!("cargo:rerun-if-changed={}", path.display());
    }
    demo_assets.push_str("];\n");
    write_if_changed(&output.join("demo_assets.rs"), demo_assets.as_bytes()).expect("Write embedded demo assets");
    let configuration = slint_build::CompilerConfiguration::new()
        .with_include_paths(vec![output])
        .with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", configuration)
        .expect("Compile the application interface");
}

fn write_if_changed(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    // Slint tracks imported generated sources. Identical rewrites would make
    // a subsequent Cargo invocation rebuild this crate unnecessarily.
    match fs::read(path) {
        Ok(existing) if existing == content => Ok(()),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => fs::write(path, content),
    }
}

fn collect_files(directory: &std::path::Path, files: &mut Vec<PathBuf>) {
    if !directory.exists() {
        return;
    }
    for entry in fs::read_dir(directory).expect("Read demo asset directory") {
        let path = entry.expect("Read demo asset").path();
        if path.is_dir() {
            collect_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}
