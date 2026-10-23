use std::path::{Path, PathBuf};

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

        compressed_payloads();
    }
}

/// Large guest files embedded in liblocaldesktop.so, xz-compressed here and
/// expanded by Portal only when the installed copy is stale (`compressed_payload!`
/// in src/android/proot/payload.rs). Paths are relative to `assets/`.
const COMPRESSED_PAYLOADS: &[&str] = &[
    "mesa-turnip/libvulkan_freedreno.so",
    "kwin-debian-arm64/libkwin.so.6.3.6",
    "kwin-anland-arm64/libkwin.so.6.3.6",
    "xwayland-portal/Xwayland",
];

fn compressed_payloads() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("payloads");
    let workers: Vec<_> = COMPRESSED_PAYLOADS
        .iter()
        .map(|asset| {
            let source = Path::new("assets").join(asset);
            println!("cargo:rerun-if-changed={}", source.display());
            let out = out_dir.join(asset);
            std::thread::spawn(move || compress_payload(&source, &out))
        })
        .collect();
    for worker in workers {
        worker.join().expect("compress payload");
    }
}

/// Writes `<out>.xz` and `<out>.rs`, the `CompressedPayload` expression for
/// it. An unchanged source keeps its earlier `.xz`: preset 9e takes a while.
fn compress_payload(source: &Path, out: &Path) {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let raw = std::fs::read(source).unwrap_or_else(|e| panic!("read {}: {e}", source.display()));
    let sha256 = format!("{:x}", Sha256::digest(&raw));
    let with_suffix = |suffix: &str| {
        let mut path = out.as_os_str().to_owned();
        path.push(suffix);
        PathBuf::from(path)
    };
    let (xz_path, stamp_path, rs_path) = (
        with_suffix(".xz"),
        with_suffix(".sha256"),
        with_suffix(".rs"),
    );
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let cached = xz_path.is_file()
        && std::fs::read_to_string(&stamp_path).is_ok_and(|stamp| stamp == sha256);
    if !cached {
        const PRESET_EXTREME: u32 = 1 << 31; // LZMA_PRESET_EXTREME, as in `xz -9e`
        let mut options =
            xz2::stream::LzmaOptions::new_preset(9 | PRESET_EXTREME).expect("xz preset");
        // A dictionary just covering the file compresses as well as preset
        // 9's 64 MiB, and the device's decoder allocates only this much.
        options.dict_size(raw.len().next_power_of_two().clamp(1 << 20, 1 << 26) as u32);
        let mut filters = xz2::stream::Filters::new();
        filters.lzma2(&options);
        let stream = xz2::stream::Stream::new_stream_encoder(&filters, xz2::stream::Check::Crc64)
            .expect("xz encoder");
        let mut encoder = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
        encoder.write_all(&raw).unwrap();
        std::fs::write(&xz_path, encoder.finish().unwrap()).unwrap();
        std::fs::write(&stamp_path, &sha256).unwrap();
    }
    std::fs::write(
        rs_path,
        format!(
            "crate::android::proot::payload::CompressedPayload {{ xz: include_bytes!({:?}), len: {}, sha256: {:?} }}\n",
            xz_path.to_str().unwrap(),
            raw.len(),
            sha256
        ),
    )
    .unwrap();
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
