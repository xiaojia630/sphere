//! ONNX Runtime's CoreML code asks `@available(macOS …)`, which compiles to calls of `__isPlatformVersionAtLeast`
//! from clang's runtime library. rustc links with `-nodefaultlibs`, so the dist build (thin LTO) can't find it:
//! link `libclang_rt.osx.a` from the clang that's installed.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let Ok(out) = std::process::Command::new("clang")
        .arg("--print-resource-dir")
        .output()
    else {
        return;
    };
    let dir = std::path::Path::new(String::from_utf8_lossy(&out.stdout).trim())
        .join("lib")
        .join("darwin");
    if dir.join("libclang_rt.osx.a").exists() {
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
}
