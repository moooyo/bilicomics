use std::{env, fs, path::PathBuf};

/// Build the pinned protocol sources for the Cargo target without network access.
pub fn build() {
    println!("cargo:rerun-if-changed=src/source/native_build.rs");
    println!("cargo:rerun-if-changed=vendor/bili-protocol");

    // The Rust adapter reports a typed capability error on unsupported platforms.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
        .join("vendor/bili-protocol");
    let wasm_source = root.join("wasm3/source");
    let cjson = root.join("cjson");

    let mut host = cc::Build::new();
    host.std("c99")
        .opt_level(2)
        .warnings(false)
        .include(root.join("host"))
        .include(&wasm_source)
        .include(&cjson)
        .define("BILIWASM_NO_MAIN", None)
        .define("d_m3GuardedMemory", Some("0"))
        .define("d_m3MaxLinearMemoryPages", Some("1024"))
        .flag("-fvisibility=hidden")
        .file(root.join("host/biliwasm.c"))
        .file(cjson.join("cJSON.c"));
    for source in [
        "m3_bind.c",
        "m3_code.c",
        "m3_compile.c",
        "m3_core.c",
        "m3_deterministic.c",
        "m3_env.c",
        "m3_exec.c",
        "m3_function.c",
        "m3_info.c",
        "m3_module.c",
        "m3_parse.c",
        "m3_validate.c",
    ] {
        host.file(wasm_source.join(source));
    }
    host.compile("biliwasm");

    let portable = root.join("portable");
    let primitives = root.join("legacy_v5");
    let mut crypto = cc::Build::new();
    crypto
        .std("c99")
        .opt_level(2)
        .warnings(false)
        .include(&portable)
        .include(&primitives)
        .include(primitives.join("vendor/mbedtls/include"))
        .include(primitives.join("vendor/mbedtls/library"))
        .define("_GNU_SOURCE", None)
        .define("MBEDTLS_CONFIG_FILE", Some("\"bili_mbedtls_config.h\""))
        .flag("-fvisibility=hidden")
        .flag("-fstack-protector-strong")
        .file(portable.join("bilicrypto.c"));
    let source_list = fs::read_to_string(primitives.join("sources.list"))
        .expect("Pinned Mbed TLS source list must be present");
    for source in source_list.lines().map(str::trim).filter(|line| !line.is_empty()) {
        crypto.file(primitives.join(source));
    }
    crypto.compile("bilicrypto");

    // wasm3 implements the floating-point operations used by the Go module.
    println!("cargo:rustc-link-lib=m");
}
