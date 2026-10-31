use cfg_aliases::cfg_aliases;

fn main() {
    // Setup cfg aliases
    cfg_aliases! {
        // Convenience aliases
        wasm_browser: { all(target_family = "wasm", target_os = "unknown") },
        // Limited POSIX platforms (not wasm)
        posix_minimal: { target_os = "espidf" },
        // Platforms where the `netdev` crate is available, i.e. everything
        // except iOS, esp-idf and wasm-in-browser. Keep in sync with the `netdev`
        // dependency target gate in Cargo.toml.
        netdev: { not(any(target_os = "ios", target_os = "espidf", all(target_family = "wasm", target_os = "unknown"))) },
        // BSD-derived platforms that share the `AF_ROUTE` routing-socket code.
        bsd: { any(target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "macos") },
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        let shim = "src/ios/path_monitor.c";
        println!("cargo:rerun-if-changed={shim}");
        // cc defaults to the installed SDK's version, which can optimize out
        // Clang's availability checks and strongly import newer APIs.
        let output = std::process::Command::new(std::env::var_os("RUSTC").unwrap())
            .args(["--print", "deployment-target", "--target"])
            .arg(std::env::var_os("TARGET").unwrap())
            .output()
            .expect("query rustc deployment target");
        assert!(
            output.status.success(),
            "could not query iOS deployment target"
        );
        let deployment = std::str::from_utf8(&output.stdout).unwrap().trim();
        let version = deployment
            .strip_prefix("IPHONEOS_DEPLOYMENT_TARGET=")
            .expect("iOS deployment target");
        println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");
        cc::Build::new()
            .env("IPHONEOS_DEPLOYMENT_TARGET", version)
            .file(shim)
            .flag("-fblocks")
            .compile("netwatch_path_monitor");
        println!("cargo:rustc-link-lib=framework=Network");
    }
}
