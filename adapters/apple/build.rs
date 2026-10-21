use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    println!("cargo:rerun-if-changed=native/Package.swift");
    // swift package update 只改 resolved 文件；漏掉它会让本地构建静默链接旧静态库
    println!("cargo:rerun-if-changed=native/Package.resolved");
    println!("cargo:rerun-if-changed=native/Sources");

    assert!(
        target_os == "macos" && target_arch == "aarch64",
        "The Apple adapter supports macOS arm64 only"
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

    // 无条件跑 swift build：SPM 自己做增量（无变动时秒级返回）。
    // 曾经的「libCAppleASR.a 存在即跳过」stamp 判断是真 bug：
    // 只改 Swift 源码（不碰 Package.swift）时会链上旧的静态库。
    //
    // 优先 swiftbuild 构建系统（Swift 6.2+）：旧 native 系统不会编译
    // mlx-swift Cmlx 里的 .metal shader，产不出 default.metallib，
    // CoreML 推理运行时必挂（CI macos-26 默认 Xcode 较老时踩中）。
    //
    // 关闭 swift-syntax 预编译包：新工具链首次构建会从 download.swift.org
    // 下载预编译宏库，SwiftPM 的下载没有超时，网络慢时整个 cargo build
    // 无限挂起；从源码构建宏库只慢在首次。不认识该参数的旧工具链本就不下载
    // 预编译包，依次回退到不带该参数、再到默认构建系统。
    // 只在全部尝试都失败时才打印输出，回退成功不留误导性的警告。
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
    // libCAppleASR.a 已包含 speech-swift 及其依赖（MLX 等）的全部对象
    println!("cargo:rustc-link-lib=static=CAppleASR");
    // 框架（对象内嵌 autolink 提示，这里显式列出关键项以保证链接顺序）
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
    // Swift 运行时 overlay（/usr/lib/swift，macOS 15+ 系统自带）。
    // 下面这份手写 dylib 列表来源：`swift build -v` 的实际链接行；
    // Xcode 升级后若出现链接错误（缺 Swift 符号），需先核对本列表是否有增删。
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
        .expect("Failed to copy mlx.metallib next to the built Apple adapter");
    // Swift 5 语言模式包的兼容钩子 + clang 运行时（___isPlatformVersionAtLeast 等）
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

/// toolchain 的 usr/lib/swift/macosx 目录。
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

/// 定位 toolchain 内的 libclang_rt.osx.a，返回其所在目录。
fn find_clang_rt_osx_dir(devdir: &Path) -> Option<PathBuf> {
    let mut candidates = vec![];
    // clang 运行时位于 Toolchains/<TC>/usr/lib/clang/...（也兼容旧布局 usr/lib/clang）
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

/// 运行构建命令；失败时返回可直接打印的输出尾部。
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
        Err(e) => Err(format!("无法执行 {:?}: {e}", cmd.get_program())),
    }
}

/// 在 dir 下递归查找指定文件名（限深，跳过 symlink 防环）。
/// 用于 SPM 产物定位：.build 布局随构建系统（swiftbuild/native）不同。
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
        // 不跟随符号链接（.build/release → out/Products/Release 会造成重复遍历）
        if p.is_dir() && !e.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            dirs.push(p);
        }
    }
    dirs.into_iter()
        .find_map(|d| find_file_recursive(&d, name, depth - 1))
}
