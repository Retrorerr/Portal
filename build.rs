use std::path::Path;

fn main() {
    desktop_stuff_manifest();

    let lib_path = "./assets/libs/arm64-v8a";
    println!("cargo::rustc-link-search={}", lib_path);

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "android" {
        cc::Build::new()
            .file("src/android/backend/wayland/wlegl_import.c")
            .flag("-std=c11")
            .flag("-Wno-unused-parameter")
            .compile("wlegl_import");

        println!("cargo:rustc-link-lib=dylib=log");
        println!("cargo:rustc-link-lib=dylib=dl");
        println!("cargo:rerun-if-changed=src/android/backend/wayland/wlegl_import.c");
    }
}

/// AAssetManager cannot list subdirectories, so the files of the `Stuff`
/// Desktop folder (assets/desktop-stuff) are listed here at build time.
fn desktop_stuff_manifest() {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read assets/desktop-stuff")
            .map(|entry| entry.expect("read assets/desktop-stuff entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
                out.push(relative.replace('\\', "/"));
            }
        }
    }
    let root = Path::new("assets/desktop-stuff");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(root, root, &mut files);
    let body: String = files
        .iter()
        .map(|file| format!("    {file:?},\n"))
        .collect();
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("desktop_stuff_files.rs");
    std::fs::write(out, format!("&[\n{body}]\n")).expect("write desktop_stuff_files.rs");
}
