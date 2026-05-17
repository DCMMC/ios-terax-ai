fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let enable_native_ios_linuxkit =
        std::env::var_os("TERAX_IOS_LINUXKIT_NATIVE").is_none_or(|value| value != "0");
    println!("cargo:rustc-check-cfg=cfg(terax_ios_linuxkit_native)");
    println!("cargo:rerun-if-env-changed=TERAX_IOS_LINUXKIT_NATIVE");
    if target.contains("apple-ios") {
        build_ios_keycommands();
    }
    if target.contains("apple-ios") && enable_native_ios_linuxkit {
        println!("cargo:rustc-cfg=terax_ios_linuxkit_native");
        build_ios_linuxkit_bridge(&target);
    }
    tauri_build::build()
}

fn build_ios_keycommands() {
    println!("cargo:rerun-if-changed=native/ios_keycommands.mm");
    cc::Build::new()
        .file("native/ios_keycommands.mm")
        .cpp(true)
        .flag("-fobjc-arc")
        .compile("terax_ios_keycommands");
}

fn build_ios_linuxkit_bridge(target: &str) {
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let linuxkit_dir = manifest_dir.join("../../ios-linuxkit");
    let platform_dir = if target.contains("sim") {
        "Debug-ApplePleaseFixFB19282108-iphonesimulator"
    } else {
        "Debug-ApplePleaseFixFB19282108-iphoneos"
    };
    let build_dir = linuxkit_dir.join("build").join(platform_dir);
    let meson_dir = build_dir.join("meson");
    let libarchive_dir = if target.contains("sim") {
        linuxkit_dir.join("deps/build/Release")
    } else {
        linuxkit_dir.join("deps/build/Release-iphoneos")
    };

    println!("cargo:rerun-if-changed=native/ios_linuxkit_bridge.c");
    println!("cargo:rerun-if-changed={}", build_dir.display());
    println!(
        "cargo:rerun-if-changed={}",
        build_dir.join("libish.a").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        meson_dir.join("libish_emu.a").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        meson_dir.join("libfakefs.a").display()
    );

    cc::Build::new()
        .file("native/ios_linuxkit_bridge.c")
        .file(linuxkit_dir.join("tools/fakefs.c"))
        .file(linuxkit_dir.join("util/fchdir.c"))
        .include(&linuxkit_dir)
        .include(linuxkit_dir.join("vdso/arm64"))
        .include(linuxkit_dir.join("app"))
        .include(linuxkit_dir.join("deps/libarchive/libarchive"))
        .define("LOG_HANDLER_NSLOG", "1")
        .define("ENGINE_ASBESTOS", "1")
        .define("GUEST_ARM64", "1")
        .flag("-fblocks")
        .compile("terax_ios_linuxkit_bridge");

    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!("cargo:rustc-link-search=native={}", meson_dir.display());
    println!(
        "cargo:rustc-link-search=native={}",
        libarchive_dir.display()
    );

    println!("cargo:rustc-link-lib=sqlite3");
    println!("cargo:rustc-link-lib=z");
    println!("cargo:rustc-link-lib=bz2");
    println!("cargo:rustc-link-lib=iconv");
    println!("cargo:rustc-link-lib=resolv");
    println!("cargo:rustc-link-lib=static=archive");
    println!("cargo:rustc-link-lib=static:+whole-archive=ish");
    println!("cargo:rustc-link-lib=static=fakefs");
    println!("cargo:rustc-link-lib=static=ish_emu");
}
