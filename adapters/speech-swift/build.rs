use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    println!("cargo:rerun-if-changed=native/Package.swift");
    // `swift package update` changes only Package.resolved; without this a local build links a stale library.
    println!("cargo:rerun-if-changed=native/Package.resolved");
    println!("cargo:rerun-if-changed=native/Sources");

    assert!(
        target_os == "macos" && target_arch == "aarch64",
        "The speech-swift adapter supports macOS arm64 only"
    );
    assert!(
        tool_available("swiftc"),
        "Swift compiler unavailable. Install Xcode and select it with xcode-select --switch /Applications/Xcode.app/Contents/Developer"
    );

    assert!(
        tool_available("metal"),
        "Metal compiler unavailable. Install Xcode, then run xcodebuild -downloadComponent MetalToolchain"
    );

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let pkg = manifest.join("native");
    let build_dir = pkg.join(".build/release");

    // Always run swift build; SwiftPM is incremental, and skipping it when the library exists
    // linked a stale library after Swift-only changes.
    //
    // Prefer the swiftbuild build system (Swift 6.2+): the older native one does not compile
    // mlx-swift's .metal shaders, so no metallib is produced and inference fails at run time.
    //
    // Disable prebuilt swift-syntax: SwiftPM downloads it without a timeout, which can hang the build
    // on a slow network; building from source is slower only the first time.
    // Older toolchains reject the flags, so fall back step by step,
    // and print output only if every attempt fails.
    let attempts: [&[&str]; 4] = [
        &[
            "--build-system",
            "swiftbuild",
            "--disable-experimental-prebuilts",
        ],
        &["--build-system", "swiftbuild"],
        &["--disable-experimental-prebuilts"],
        &[],
    ];
    let mut reports = Vec::new();
    let ok = attempts.iter().any(|extra| {
        match run(Command::new("swift")
            .args(["build", "-c", "release"])
            .args(*extra)
            .current_dir(&pkg))
        {
            Ok(()) => true,
            Err(report) => {
                reports.push(report);
                false
            }
        }
    });
    if !ok {
        for report in reports {
            for line in report.lines() {
                println!("cargo:warning={line}");
            }
        }
        panic!(
            "Swift build failed. Install Xcode and its Metal compiler with xcodebuild -downloadComponent MetalToolchain; see the build output above"
        );
    }

    println!("cargo:rustc-link-search=native={}", build_dir.display());
    // libCAppleASR.a contains speech-swift and its dependencies, including MLX.
    println!("cargo:rustc-link-lib=static=CAppleASR");
    // Frameworks, listed explicitly to fix the link order.
    for fw in [
        "Foundation",
        "CoreML",
        "Metal",
        "Accelerate",
        "CoreFoundation",
        "AVFoundation",
        "AVFAudio",
        "AppKit",
        "CoreAudio",
        "CryptoKit",
        "NaturalLanguage",
        "Network",
        "Security",
        "CoreGraphics",
    ] {
        println!("cargo:rustc-link-lib=framework={fw}");
    }
    // Swift runtime overlays in /usr/lib/swift (part of macOS 15+).
    // The list comes from the link line of `swift build -v`;
    // check it first when a new Xcode reports missing Swift symbols.
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    for dylib in [
        "swiftCore",
        "swift_Concurrency",
        "swift_Builtin_float",
        "swift_errno",
        "swiftAccelerate",
        "swiftAVFoundation",
        "swiftCoreAudio",
        "swiftCoreFoundation",
        "swiftCoreImage",
        "swiftCoreMIDI",
        "swiftDarwin",
        "swiftDispatch",
        "swiftIOKit",
        "swiftMetal",
        "swiftNaturalLanguage",
        "swiftObjectiveC",
        "swiftObservation",
        "swiftos",
        "swiftOSLog",
        "swiftQuartzCore",
        "swiftRegexBuilder",
        "swiftsimd",
        "swiftSpatial",
        "swift_StringProcessing",
        "swiftUniformTypeIdentifiers",
        "swiftXPC",
    ] {
        println!("cargo:rustc-link-lib=dylib={dylib}");
    }
    println!("cargo:rustc-link-lib=dylib=c++");
    println!("cargo:rustc-link-lib=dylib=objc");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    // MLX loads mlx.metallib colocated with the executable.
    let products = build_dir.canonicalize().unwrap_or(build_dir);
    let expected = products.join("mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib");
    let metallib = if expected.is_file() {
        expected
    } else {
        find_file_recursive(&pkg.join(".build"), "default.metallib", 8).expect(
            "Swift build produced no MLX metallib. Install Xcode and run xcodebuild -downloadComponent MetalToolchain, then rebuild",
        )
    };
    // OUT_DIR = <target>/<profile>/build/<package>-<hash>/out.
    let exe_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("../../..");
    std::fs::copy(metallib, exe_dir.join("mlx.metallib"))
        .expect("Failed to copy mlx.metallib next to the built speech-swift adapter");
    // Compatibility hooks for Swift 5 mode packages and the clang runtime (___isPlatformVersionAtLeast and others).
    let devdir = xcode_devdir();
    if let Some(swift_lib) = devdir.as_deref().and_then(toolchain_swift_lib_dir) {
        println!("cargo:rustc-link-search=native={}", swift_lib.display());
        println!("cargo:rustc-link-lib=static=swiftCompatibility56");
    }
    if let Some(rt) = devdir.as_deref().and_then(find_clang_rt_osx_dir) {
        println!("cargo:rustc-link-search=native={}", rt.display());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
}

fn xcode_devdir() -> Option<PathBuf> {
    let out = Command::new("xcode-select").arg("-p").output().ok()?;
    out.status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// The toolchain's usr/lib/swift/macosx directory.
fn toolchain_swift_lib_dir(devdir: &Path) -> Option<PathBuf> {
    if let Ok(rd) = std::fs::read_dir(devdir.join("Toolchains")) {
        for e in rd.flatten() {
            let d = e.path().join("usr/lib/swift/macosx");
            if d.join("libswiftCompatibility56.a").is_file() {
                return Some(d);
            }
        }
    }
    None
}

/// The directory of the toolchain's libclang_rt.osx.a.
fn find_clang_rt_osx_dir(devdir: &Path) -> Option<PathBuf> {
    let mut candidates = vec![];
    // Toolchains/<TC>/usr/lib/clang/..., or usr/lib/clang in older layouts
    if let Ok(rd) = std::fs::read_dir(devdir.join("Toolchains")) {
        for e in rd.flatten() {
            collect_a_files(&e.path().join("usr/lib/clang"), &mut candidates);
        }
    }
    collect_a_files(&devdir.join("usr/lib/clang"), &mut candidates);
    for p in candidates {
        let is_it = p.to_string_lossy().contains("darwin")
            && p.file_name().and_then(|s| s.to_str()) == Some("libclang_rt.osx.a");
        if is_it {
            return p.parent().map(Path::to_path_buf);
        }
    }
    None
}

fn collect_a_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_a_files(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("a") {
                out.push(p);
            }
        }
    }
}

fn tool_available(name: &str) -> bool {
    Command::new("xcrun")
        .args(["--find", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Runs a build command; on failure returns the end of its output.
fn run(cmd: &mut Command) -> Result<(), String> {
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "{:?} exited with {:?} (stdout tail):\n{}\n(stderr tail):\n{}",
            cmd.get_program(),
            out.status.code(),
            tail(&String::from_utf8_lossy(&out.stdout), 40),
            tail(&String::from_utf8_lossy(&out.stderr), 40),
        )),
        Err(e) => Err(format!("cannot run {:?}: {e}", cmd.get_program())),
    }
}

/// Finds a file by name under dir, with a depth limit and without following symlinks,
/// because the .build layout differs between build systems.
fn find_file_recursive(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut dirs = vec![];
    for e in entries.flatten() {
        let p = e.path();
        if e.file_name() == name && p.is_file() {
            return Some(p);
        }
        // .build/release links to out/Products/Release; following it would visit files twice.
        if p.is_dir() && !e.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            dirs.push(p);
        }
    }
    dirs.into_iter()
        .find_map(|d| find_file_recursive(&d, name, depth - 1))
}
