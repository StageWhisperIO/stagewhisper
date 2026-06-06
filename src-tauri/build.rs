fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_swift_glass)");
    println!("cargo::rustc-check-cfg=cfg(has_swift_microphone)");

    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-lib=framework=AVFoundation");

    #[cfg(target_os = "macos")]
    compile_swift();

    #[cfg(target_os = "macos")]
    compile_swift_microphone();

    tauri_build::build();
}

#[cfg(target_os = "macos")]
fn compile_swift() {
    use std::path::PathBuf;
    use std::process::Command;

    let swift_src = "swift/GlassControlBar.swift";

    if !std::path::Path::new(swift_src).exists() {
        println!(
            "cargo:warning=Swift source not found at {swift_src}, skipping glass buttons (HTML fallback)"
        );
        return;
    }

    println!("cargo:rerun-if-changed={swift_src}");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let sdk_path = run_xcrun(&["--show-sdk-path"]);

    let target = std::env::var("TARGET").unwrap_or_default();
    let arch = if target.starts_with("aarch64") {
        "arm64"
    } else if target.starts_with("x86_64") {
        "x86_64"
    } else {
        "arm64"
    };

    let swift_target = format!("{arch}-apple-macos15.0");

    let obj_path = out_dir.join("glass_control_bar.o");

    let status = Command::new("swiftc")
        .args([
            "-c",
            "-parse-as-library",
            "-module-name",
            "SWGlassControlBar",
            "-sdk",
            &sdk_path,
            "-target",
            &swift_target,
            "-O",
            "-whole-module-optimization",
            "-o",
        ])
        .arg(&obj_path)
        .arg(swift_src)
        .status();

    match status {
        Ok(s) if s.success() => { /* compiled OK */ }
        Ok(s) => {
            println!(
                "cargo:warning=swiftc exited with {s}; glass buttons unavailable (HTML fallback). \
                 Expected only if Xcode 26 / macOS Tahoe SDK is missing or the macro plugin server was blocked."
            );
            return;
        }
        Err(e) => {
            println!("cargo:warning=failed to run swiftc ({e}); glass buttons unavailable (HTML fallback).");
            return;
        }
    }

    let lib_path = out_dir.join("libswglass.a");
    let ar_status = Command::new("ar")
        .arg("rcs")
        .arg(&lib_path)
        .arg(&obj_path)
        .status()
        .expect("failed to run `ar`");
    assert!(ar_status.success(), "`ar` failed to create static library");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=swglass");

    if let Some(swift_lib_dir) = find_swift_lib_dir() {
        println!("cargo:rustc-link-search=native={}", swift_lib_dir.display());
    }

    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    let sdk_swift_lib = format!("{}/usr/lib/swift", sdk_path);
    if std::path::Path::new(&sdk_swift_lib).exists() {
        println!("cargo:rustc-link-search=native={}", sdk_swift_lib);
    }

    for framework in &["SwiftUI", "AppKit", "Foundation", "Combine"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }

    for lib in &[
        "swiftCore",
        "swift_Concurrency",
        "swiftObjectiveC",
        "swiftCoreFoundation",
        "swiftDispatch",
        "swiftCoreGraphics",
        "swiftDarwin",
        "swiftObservation",
    ] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }

    println!("cargo:rustc-cfg=has_swift_glass");
}

#[cfg(target_os = "macos")]
fn run_xcrun(args: &[&str]) -> String {
    let output = std::process::Command::new("xcrun")
        .args(args)
        .output()
        .expect("failed to run xcrun — is Xcode installed?");
    String::from_utf8(output.stdout)
        .expect("xcrun produced non-UTF-8 output")
        .trim()
        .to_string()
}

/// Locate the Swift standard library directory inside the active Xcode toolchain.
///
/// Path is typically: `<Xcode>/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx`
#[cfg(target_os = "macos")]
fn find_swift_lib_dir() -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("xcrun")
        .args(["--toolchain", "default", "-f", "swiftc"])
        .output()
        .ok()?;
    let swiftc_path = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let swift_lib_dir = std::path::PathBuf::from(&swiftc_path)
        .parent()? // bin/
        .parent()? // usr/
        .join("lib/swift/macosx");
    if swift_lib_dir.exists() {
        Some(swift_lib_dir)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn compile_swift_microphone() {
    use std::path::PathBuf;
    use std::process::Command;

    let swift_src = "swift/MicrophoneBridge.swift";

    if !std::path::Path::new(swift_src).exists() {
        eprintln!(
            "warning: Swift source not found at {swift_src}, skipping microphone bridge compilation"
        );
        return;
    }

    println!("cargo:rerun-if-changed={swift_src}");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let sdk_path = run_xcrun(&["--show-sdk-path"]);

    let target = std::env::var("TARGET").unwrap_or_default();
    let arch = if target.starts_with("aarch64") {
        "arm64"
    } else if target.starts_with("x86_64") {
        "x86_64"
    } else {
        "arm64"
    };

    let swift_target = format!("{arch}-apple-macos13.0");

    let obj_path = out_dir.join("microphone_bridge.o");

    let status = Command::new("swiftc")
        .args([
            "-c",
            "-parse-as-library",
            "-module-name",
            "SWMicrophoneBridge",
            "-sdk",
            &sdk_path,
            "-target",
            &swift_target,
            "-O",
            "-whole-module-optimization",
            "-o",
        ])
        .arg(&obj_path)
        .arg(swift_src)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("warning: swiftc exited with {s}; microphone bridge will not be available.");
            return;
        }
        Err(e) => {
            eprintln!(
                "warning: failed to run swiftc ({e}); microphone bridge will not be available."
            );
            return;
        }
    }

    let lib_path = out_dir.join("libswmicrophone.a");
    let ar_status = Command::new("ar")
        .arg("rcs")
        .arg(&lib_path)
        .arg(&obj_path)
        .status()
        .expect("failed to run `ar`");
    assert!(ar_status.success(), "`ar` failed to create static library");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=swmicrophone");

    println!("cargo:rustc-link-lib=framework=AVFoundation");

    println!("cargo:rustc-cfg=has_swift_microphone");
}
