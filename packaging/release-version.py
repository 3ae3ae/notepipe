#!/usr/bin/env python3
"""Resolve a published version for manual, tag, or workflow-dispatch releases."""
import json
import os
import re
import urllib.request

# 与 packaging/homebrew/update.py、packaging/aur/update-aur.sh 同一校验口径：
# semver.org 官方文法（核心版本拒绝前导零，如 01.2.3；预发布标识符按官方
# 规范，如 1.2.3-rc.1）。发布版本不使用 build 元数据（+build 段），三方统一
# 拒绝。预发布通道仅限 alpha/beta/rc.N（N ≥ 1），与发布矩阵一致。
PRERELEASE_CHANNELS = ("alpha", "beta", "rc")
SEMVER = re.compile(
    r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?"
)


def parse_release(version):
    """Return (channel, number) for a release version, or None if invalid."""
    match = SEMVER.fullmatch(version)
    if match is None:
        return None
    prerelease = match[4]
    if prerelease is None:
        return "stable", None
    channel, dot, number = prerelease.partition(".")
    if channel not in PRERELEASE_CHANNELS or not dot or not number.isdigit() or int(number) < 1:
        return None
    return channel, number


def api(path):
    request = urllib.request.Request(
        f"https://api.github.com/repos/{os.environ['GITHUB_REPOSITORY']}/{path}",
        headers={"Authorization": f"Bearer {os.environ['GH_TOKEN']}",
                 "Accept": "application/vnd.github+json"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def main():
    version = os.environ.get("REQUESTED_VERSION", "")
    if not version:
        branch = os.environ.get("RELEASE_BRANCH", "")
        if branch.startswith("v") and parse_release(branch[1:]):
            version = branch
        else:
            sha = os.environ.get("RELEASE_SHA", "")
            releases = api("releases?per_page=100")
            matches = [r for r in releases if sha and r["target_commitish"] == sha
                       and not r["draft"]]
            if len(matches) != 1:
                raise SystemExit("Cannot identify one published release for the triggering commit")
            version = matches[0]["tag_name"]
    version = version.removeprefix("v")
    match = parse_release(version)
    if not match:
        raise SystemExit("Expected a version such as 1.7.0 or 2.0.0-alpha.1")
    channel = match[0]
    release = api(f"releases/tags/v{version}")
    if release["draft"]:
        raise SystemExit("Package channels cannot publish draft releases")
    if release["prerelease"] != (channel != "stable"):
        raise SystemExit("Release prerelease flag does not match its version")
    publish = channel == "stable" or os.environ.get("ALLOW_PRERELEASE") == "true"
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"version={version}\nchannel={channel}\npublish={str(publish).lower()}\n")
    if publish:
        print(f"Updating {channel} GUI and CLI packages to {version}")
    else:
        print(f"Skipping prerelease {version}: this package channel only publishes stable releases")


if __name__ == "__main__":
    main()
