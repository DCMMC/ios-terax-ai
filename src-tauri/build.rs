fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let enable_native_ios_linuxkit =
        std::env::var_os("TERAX_IOS_LINUXKIT_NATIVE").is_some_and(|value| value != "0");
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

    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!("cargo:rustc-link-search=native={}", meson_dir.display());
    println!("cargo:rustc-link-search=native={}", deps_dir.display());

    println!("cargo:rustc-link-lib=sqlite3");

    println!("cargo:rustc-link-arg=-Wl,-ld_classic");
    println!("cargo:rustc-link-arg=-Wl,-sectalign,__DATA,__percpu_first,1000");
    println!("cargo:rustc-link-arg=-Wl,-sectalign,__DATA,__tracepoints,20");
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        meson_dir.join("libish_emu.a").display()
    );
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        meson_dir.join("libfakefs.a").display()
    );
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        build_dir.join("liblinux.a").display()
    );
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        build_dir.join("libiSHLinux.a").display()
    );
}
