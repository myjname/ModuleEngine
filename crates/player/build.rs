fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let host = std::env::var("HOST").expect("Cargo HOST");
    let target = std::env::var("TARGET").expect("Cargo TARGET");
    assert_eq!(
        host, target,
        "MODULE_ENGINE_NATIVE_TARGET_REQUIRED: Player requires HOST=TARGET; remove a foreign build.target/CARGO_BUILD_TARGET"
    );
    // Shared Cargo product collector uses the same proof keys for Custom/Player.
    println!("cargo:rustc-env=MODULE_ENGINE_WORKER_BUILD_HOST={host}");
    println!("cargo:rustc-env=MODULE_ENGINE_WORKER_BUILD_TARGET={target}");
}
