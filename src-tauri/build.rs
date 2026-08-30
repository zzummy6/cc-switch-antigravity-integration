fn main() {
    tauri_build::build();

    // Windows: Embed Common Controls v6 manifest for test binaries
    //
    // When running `cargo test`, the generated test executables don't include
    // the standard Tauri application manifest. Without Common Controls v6,
    // `tauri::test` calls fail with STATUS_ENTRYPOINT_NOT_FOUND.
    //
    // MSVC: embed via /MANIFEST:EMBED (link.exe args).
    // GNU:  ld rejects /MANIFEST:*; embed via windres-generated COFF instead
    //       (best-effort: silently skipped when windres is unavailable).
    //
    // 注意：build script 以 host 三元组编译，`#[cfg(target_env=...)]` 判断的是
    // host 而非构建目标，这里必须用 CARGO_CFG_TARGET_* 环境变量。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    let manifest_path = manifest_path();

    if target_env == "msvc" {
        let manifest_arg = format!("/MANIFESTINPUT:{}", manifest_path.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg={}", manifest_arg);
        // Avoid duplicate manifest resources in binary builds.
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
    } else if target_env == "gnu" {
        let out_dir =
            std::path::PathBuf::from(std::env::var("OUT_DIR").expect("missing OUT_DIR"));
        let coff = out_dir.join("common_controls.coff");
        let rc = out_dir.join("common_controls.rc");

        // rc 路径必须用正斜杠（windres 不认 Windows 反斜杠转义）
        let manifest_forward = manifest_path.display().to_string().replace('\\', "/");
        let rc_source = format!("1 24 \"{manifest_forward}\"\n");
        std::fs::write(&rc, rc_source).expect("write common-controls rc");

        let windres = ["x86_64-w64-mingw32-windres", "windres"]
            .iter()
            .find(|candidate| which_tool(candidate));
        let linked = windres
            .and_then(|tool| {
                std::process::Command::new(tool)
                    .args([
                        "--input",
                        rc.to_str().expect("rc path"),
                        "--output",
                        coff.to_str().expect("coff path"),
                        "--output-format=coff",
                    ])
                    .status()
                    .ok()
                    .map(|status| status.success())
            })
            .unwrap_or(false);

        if linked && coff.exists() {
            println!("cargo:rustc-link-arg={}", coff.display());
        } else {
            // 没有 windres：跳过嵌入（tauri::test 在 GNU 下会失败，普通逻辑测试不受影响）
            println!(
                "cargo:warning=windres unavailable; Common Controls manifest not embedded (GNU target)"
            );
        }
    }

    println!("cargo:rerun-if-changed={}", manifest_path.display());
}

fn manifest_path() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"),
    )
    .join("common-controls.manifest")
}

fn which_tool(name: &str) -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    let candidates = [name.to_string(), format!("{name}.exe")];
    std::env::split_paths(&path).any(|dir| {
        candidates
            .iter()
            .any(|candidate| dir.join(candidate).is_file())
    })
}
