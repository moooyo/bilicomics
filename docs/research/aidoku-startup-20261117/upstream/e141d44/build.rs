use std::{env, fs, io::Read, path::PathBuf};

const BUNDLED_FONT_FILES: &[&str] = &[
    "NotoSansSC-Regular.otf",
    "NotoSansSC-Medium.otf",
    "NotoSansSC-Bold.otf",
    "NotoSerifSC-Regular.otf",
    "NotoSerifSC-Bold.otf",
    "NotoSerifSC-Black.otf",
    "Literata-Regular.ttf",
    "Literata-SemiBold.ttf",
    "Literata-Bold.ttf",
];

fn prepare_bundled_fonts(output: &std::path::Path) {
    let directory = output.join("fonts");
    fs::create_dir_all(&directory).expect("Create generated font directory");
    for name in BUNDLED_FONT_FILES {
        let input = format!("assets/fonts/{name}.gz");
        println!("cargo:rerun-if-changed={input}");
        let compressed = fs::File::open(&input).expect("Read a bundled font source");
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(compressed)
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .expect("Expand a bundled font at build time");
        assert!(bytes.len() <= 64 * 1024 * 1024, "Bundled font is too large");
        let target = directory.join(name);
        if fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
            fs::write(target, bytes).expect("Write a generated font payload");
        }
    }
}

fn main() {
    if env::var_os("CARGO_FEATURE_KINDLE").is_some()
        && env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "linux")
    {
        println!("cargo:rerun-if-changed=src/native_media/codec.c");
        println!("cargo:rerun-if-changed=scripts/native-codec-probe.c");
        println!("cargo:rerun-if-changed=scripts/native-codec-bench.h");
        cc::Build::new()
            .file("src/native_media/codec.c")
            .std("c11")
            .opt_level(2)
            .warnings(true)
            .compile("aidoku_jpeg_worker");
    }
    if env::var_os("CARGO_FEATURE_NATIVE_MEDIA_BENCH").is_some() {
        assert_eq!(
            env::var("CARGO_CFG_TARGET_OS").unwrap(),
            "linux",
            "Native media benchmarks require the designated Linux verification environment"
        );
        println!("cargo:rerun-if-changed=scripts/native-codec-probe.c");
        println!("cargo:rerun-if-changed=scripts/native-codec-bench.h");
        cc::Build::new()
            .file("scripts/native-codec-probe.c")
            .define("AIDOKU_CODEC_BENCH_LIBRARY", None)
            .std("c11")
            .opt_level(2)
            .warnings(true)
            .compile("aidoku_codec_bench");
    }
    for locale in ["zh-CN", "zh-TW", "en", "ja"] {
        println!("cargo:rerun-if-changed=assets/i18n/{locale}.json");
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    prepare_bundled_fonts(&output);
    let labels: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(
        &fs::read("assets/i18n/zh-CN.json").expect("Read interface translations"),
    )
    .expect("Parse interface translations");
    let mut translations = String::from(
        "export global Strings {\n    in-out property <string> locale-code: \"zh-CN\";\n",
    );
    let mut setters = String::from(
        "pub fn apply_ui_labels(ui: &crate::ui::AppWindow) -> bool {\n\
         use slint::ComponentHandle;\n\
         let labels = ui.global::<crate::ui::Strings>();\n\
         let code = language();\n\
         if labels.get_locale_code().as_str() == code { return false; }\n",
    );
    for (key, value) in labels {
        let value = serde_json::to_string(value.as_str().expect("Translation must be text"))
            .expect("Escape interface translation");
        translations.push_str(&format!(
            "    in-out property <string> {}: {};\n",
            key.replace('_', "-"),
            value
        ));
        setters.push_str(&format!(
            "    labels.set_{key}(translate({key:?}).into());\n"
        ));
    }
    translations.push_str("}\n");
    setters.push_str("    labels.set_locale_code(code.into());\n    true\n}\n");
    fs::write(output.join("strings.slint"), translations).expect("Write generated translations");
    fs::write(output.join("translation_setters.rs"), setters)
        .expect("Write generated translation setters");
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new()
            .with_include_paths(vec![output])
            .with_style("fluent".into()),
    )
    .expect("Compile the application interface");
}
