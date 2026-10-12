#!/usr/bin/env python3
"""Build and package the native app and its matching CLI on the current platform."""
import argparse
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import tomllib
import urllib.request

from dmg import build_install_dmg

ROOT = Path(__file__).resolve().parents[1]
PROJECT = ROOT.parent


def macos_versions(version):
    """Keep the product SemVer while using Apple's bundle version syntax."""
    match = re.fullmatch(r"(\d+\.\d+\.\d+)(?:-(alpha|beta|rc)\.([1-9]\d*))?", version)
    if not match:
        raise ValueError(f"Unsupported macOS release version: {version}")
    short, channel, number = match.groups()
    build = short
    if channel:
        if int(number) > 255:
            raise ValueError("Apple prerelease build numbers must be in 1..255")
        build += {"alpha": "a", "beta": "b", "rc": "fc"}[channel] + number
    return {"CFBundleShortVersionString": short, "CFBundleVersion": build,
            "Course2mdVersion": version}


def run(*args):
    subprocess.run(args, cwd=PROJECT, check=True)


def ytdlp_asset(system, machine):
    """Standalone binary asset name in yt-dlp's GitHub releases, or None."""
    if system == "Darwin":
        return "yt-dlp_macos"
    if system == "Windows":
        return {"AMD64": "yt-dlp.exe", "ARM64": "yt-dlp_arm64.exe"}.get(machine)
    if system == "Linux":
        return {"x86_64": "yt-dlp_linux", "aarch64": "yt-dlp_linux_aarch64"}.get(machine)
    return None


def bundle_ytdlp(dest):
    """Place a standalone yt-dlp beside the app binaries.

    COURSE2MD_YTDLP points at a local build to bundle instead of downloading.
    Failure is non-fatal: the app still resolves yt-dlp from PATH at runtime.
    """
    try:
        local = os.environ.get("COURSE2MD_YTDLP")
        if local:
            shutil.copy2(local, dest)
        else:
            asset = ytdlp_asset(platform.system(), platform.machine())
            if not asset:
                return None
            url = f"https://github.com/yt-dlp/yt-dlp/releases/latest/download/{asset}"
            tmp = dest.parent / (dest.name + ".download")
            try:
                with urllib.request.urlopen(url, timeout=300) as response, tmp.open("wb") as out:
                    shutil.copyfileobj(response, out)
                if tmp.stat().st_size < 1_000_000:
                    raise OSError(f"{asset} download is suspiciously small")
                tmp.replace(dest)
            finally:
                tmp.unlink(missing_ok=True)
        dest.chmod(0o755)
        return dest
    except OSError as error:
        print(f"warning: yt-dlp not bundled ({error}); the app will use yt-dlp from PATH")
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args()
    system = platform.system()
    # Fail before expensive builds if the platform's packaging tools are absent.
    try:
        if system == "Darwin":
            import dmgbuild  # noqa: F401
        elif system == "Windows":
            from verify_windows_icon import verify_windows_icon
    except ImportError as error:
        raise SystemExit("Install packaging tools: python -m pip install -r "
                         "desktop/scripts/requirements-packaging.txt") from error
    profile = "debug" if args.debug else "release"
    flags = [] if args.debug else ["--release", "--locked"]
    revisions = {
        name: subprocess.check_output(
            ["git", "-C", str(ROOT / ".deps" / name), "rev-parse", "HEAD"], text=True
        ).strip()
        for name in ("zed", "component")
    }
    if not args.debug:
        if not (ROOT / "sources.lock.json").is_file():
            raise SystemExit("Freeze the tested source revisions with scripts/sources.py --freeze before release")
        expected = json.loads((ROOT / "sources.lock.json").read_text())
        if revisions != expected:
            raise SystemExit("Prepared sources differ from the tested release revisions; prepare sources with --locked")
    if not args.no_build:
        run("cargo", "build", *flags)
        run("cargo", "build", "--manifest-path", str(ROOT / "Cargo.toml"), *flags)
    version = tomllib.loads((PROJECT / "Cargo.toml").read_text())["package"]["version"]
    suffix = ".exe" if system == "Windows" else ""
    package_name = f"course2md-desktop-{system.lower()}-{platform.machine()}"
    base = ROOT / "target" / "packages" / package_name
    if base.exists():
        shutil.rmtree(base)
    base.mkdir(parents=True, exist_ok=True)
    if system == "Darwin":
        bundle = base / "course2md.app"
        binaries = bundle / "Contents/MacOS"
        resources = bundle / "Contents/Resources"
        binaries.mkdir(parents=True, exist_ok=True)
        resources.mkdir(parents=True, exist_ok=True)
        with (bundle / "Contents/Info.plist").open("wb") as stream:
            plistlib.dump({
                "CFBundleName": "course2md", "CFBundleDisplayName": "course2md",
                "CFBundleIdentifier": "dev.course2md.desktop",
                "CFBundleExecutable": "course2md-desktop", "CFBundlePackageType": "APPL",
                **macos_versions(version),
                "NSHighResolutionCapable": True, "NSPrincipalClass": "NSApplication",
                "LSMinimumSystemVersion": "15.0", "CFBundleIconFile": "course2md.icns",
            }, stream)
        shutil.copy2(ROOT / "assets/icon.icns", resources / "course2md.icns")
        # MLX first searches beside the executable for mlx.metallib. Keeping
        # that layout avoids changing the CLI's working directory and breaking
        # user-supplied relative source, output, and model paths.
        metal = PROJECT / "target" / profile / "mlx.metallib"
        if metal.is_file():
            shutil.copy2(metal, resources / "mlx.metallib")
            (binaries / "mlx.metallib").symlink_to("../Resources/mlx.metallib")
        elif platform.machine() == "arm64" and not os.environ.get("COURSE2MD_NO_APPLE"):
            raise SystemExit("Apple speech library was built without mlx.metallib; inspect the core build output")
    else:
        binaries = base
        if system == "Linux":
            (base / "course2md.desktop").write_text(
                "[Desktop Entry]\nType=Application\nName=course2md\nComment=Turn courses into illustrated notes\n"
                "Exec=course2md-desktop\nIcon=course2md\nTerminal=false\nCategories=Education;AudioVideo;\n")
            shutil.copy2(ROOT / "assets/icon.png", base / "course2md.png")
    shutil.copy2(PROJECT / "target" / profile / f"course2md{suffix}", binaries / f"course2md{suffix}")
    shutil.copy2(ROOT / "target" / profile / f"course2md-desktop{suffix}", binaries / f"course2md-desktop{suffix}")
    # 自带 yt-dlp：与可执行文件同目录是运行时搜索清单的一档；COURSE2MD_NO_YTDLP=1 可跳过。
    if not os.environ.get("COURSE2MD_NO_YTDLP"):
        bundled = bundle_ytdlp(binaries / f"yt-dlp{suffix}")
        if bundled:
            print(f"bundled {bundled.name}")
    if system == "Windows":
        verify_windows_icon(binaries / "course2md-desktop.exe")
    shutil.copy2(PROJECT / "LICENSE", base / "LICENSE")
    shutil.copy2(ROOT / "assets/material/LICENSE", base / "LICENSE-material-icons")
    shutil.copytree(ROOT / "assets/themes/licenses", base / "design-licenses", dirs_exist_ok=True)
    shutil.copy2(ROOT / "README.md", base / "README.md")
    # A development archive follows main and must not claim the previous release
    # revisions. Release builds have already checked this snapshot against the lock.
    (base / "sources.lock.json").write_text(json.dumps(revisions, indent=2) + "\n")
    if system == "Darwin":
        shutil.copy2(PROJECT / "LICENSE", resources / "LICENSE")
        shutil.copy2(ROOT / "assets/material/LICENSE", resources / "LICENSE-material-icons")
        shutil.copytree(ROOT / "assets/themes/licenses", resources / "design-licenses", dirs_exist_ok=True)
        shutil.copy2(base / "sources.lock.json", resources / "sources.lock.json")
        identity = os.environ.get("APPLE_SIGNING_IDENTITY", "-")
        signing = ["--force", "--sign", identity]
        if identity != "-":
            signing.extend(["--options", "runtime", "--timestamp"])
        run("codesign", *signing, str(binaries / "course2md"))
        run("codesign", *signing, str(binaries / "course2md-desktop"))
        # 自带 yt-dlp 作为嵌套代码一并签名（--deep verify 与公证要求包内代码均可校验）
        ytdlp = binaries / "yt-dlp"
        if ytdlp.is_file():
            run("codesign", *signing, str(ytdlp))
        run("codesign", *signing, str(bundle))
        run("codesign", "--verify", "--deep", "--strict", str(bundle))
        notarization = [os.environ.get(key) for key in ("APPLE_API_KEY_PATH", "APPLE_API_KEY_ID", "APPLE_API_ISSUER")]
        if identity != "-" and not all(notarization):
            raise SystemExit(
                "已设置签名身份但公证材料不全（APPLE_API_KEY_PATH/APPLE_API_ISSUER/APPLE_API_ID），"
                "产出的 DMG 会被 Gatekeeper 拦截；请补全材料或改用 '-' 跳过签名"
            )
        if identity != "-" and all(notarization):
            upload = base.parent / "notarization.zip"
            run("ditto", "-c", "-k", "--keepParent", str(bundle), str(upload))
            run("xcrun", "notarytool", "submit", str(upload), "--key", notarization[0],
                "--key-id", notarization[1], "--issuer", notarization[2], "--wait")
            run("xcrun", "stapler", "staple", str(bundle))
            upload.unlink()
        dmg = base.parent / f"course2md-gui-macos-{platform.machine()}.dmg"
        if dmg.exists():
            dmg.unlink()
        build_install_dmg(bundle, dmg)
        if identity != "-":
            run("codesign", "--force", "--sign", identity, "--timestamp", str(dmg))
        if identity != "-" and all(notarization):
            run("xcrun", "notarytool", "submit", str(dmg), "--key", notarization[0],
                "--key-id", notarization[1], "--issuer", notarization[2], "--wait")
            run("xcrun", "stapler", "staple", str(dmg))
        print(dmg)
    if system == "Darwin":
        archive = str(base) + ".zip"
        # ditto preserves the signed app's symlinks and resource metadata.
        run("ditto", "-c", "-k", "--keepParent", str(base), archive)
    else:
        archive = shutil.make_archive(str(base), "zip" if system == "Windows" else "gztar", base.parent, base.name)
    print(archive)


if __name__ == "__main__":
    main()
