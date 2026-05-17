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
        "DebugLinux-iphonesimulator"
    } else {
        "DebugLinux-iphoneos"
    };
    let build_dir = linuxkit_dir.join("build").join(platform_dir);
    let meson_dir = build_dir.join("meson");
    let deps_dir = meson_dir.join("deps");

    println!("cargo:rerun-if-changed=native/ios_linuxkit_bridge.c");
    println!("cargo:rerun-if-changed={}", build_dir.display());

    cc::Build::new()
        .file("native/ios_linuxkit_bridge.c")
        .include(linuxkit_dir.join("app"))
        .flag("-fblocks")
        .compile("terax_ios_linuxkit_bridge");

    cc::Build::new()
        .file("native/ios_linuxkit_emu_arm64.c")
        .include(&linuxkit_dir)
        .include(deps_dir.join("linux/include"))
        .include(deps_dir.join("linux/arch/ish/include/generated"))
        .include(linuxkit_dir.join("deps/linux/arch/ish/kernel"))
        .include(linuxkit_dir.join("deps/linux/arch/ish/include"))
        .include(linuxkit_dir.join("deps/linux/include"))
        .include(linuxkit_dir.join("deps"))
        .define("GUEST_ARM64", "1")
        .define("ENGINE_ASBESTOS", "1")
        .flag("-include")
        .flag("user.h")
        .flag("-include")
        .flag("linux/kconfig.h")
        .compile("terax_ios_linuxkit_emu_arm64");

    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!("cargo:rustc-link-search=native={}", meson_dir.display());
    println!("cargo:rustc-link-search=native={}", deps_dir.display());

    println!("cargo:rustc-link-lib=sqlite3");
    println!("cargo:rustc-link-lib=static=iSHApp");
    println!("cargo:rustc-link-lib=static:+whole-archive=iSHLinux");
    println!("cargo:rustc-link-lib=static:+whole-archive=linux");
    println!("cargo:rustc-link-lib=static=fakefs");
    println!("cargo:rustc-link-lib=static=ish_emu");

    println!("cargo:rustc-link-arg=-Wl,-ld_classic");
    println!("cargo:rustc-link-arg=-Wl,-sectalign,__DATA,__percpu_first,1000");
    println!("cargo:rustc-link-arg=-Wl,-sectalign,__DATA,__tracepoints,20");
}
