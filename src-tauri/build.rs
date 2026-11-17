fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let source = "native/island_glass";
        println!("cargo:rerun-if-changed={source}");
        let mut build = cc::Build::new();
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            // Match the existing statically linked sherpa-onnx Windows runtime.
            build.static_crt(true).flag("/EHsc");
        }
        build
            .cpp(true)
            .std("c++17")
            .define("NOMINMAX", None)
            .define("WIN32_LEAN_AND_MEAN", None)
            .file(format!("{source}/island_glass.cc"))
            .compile("agenthud_island_glass");
        for library in [
            "d3d11",
            "d3dcompiler",
            "dcomp",
            "dxgi",
            "dwmapi",
            "user32",
            "ole32",
        ] {
            println!("cargo:rustc-link-lib={library}");
        }
    }
    tauri_build::build()
}
