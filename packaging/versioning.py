#!/usr/bin/env python3
"""Release version grammar shared by every package channel.

semver.org core and prerelease grammar without build metadata, limited to the
alpha/beta/rc.N (N >= 1) prerelease channels that the release matrix publishes.
Run as a script to validate one version: exit status 0 when it is accepted.
"""
import re
import sys

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


if __name__ == "__main__":
    sys.exit(0 if len(sys.argv) == 2 and parse_release(sys.argv[1]) else 1)
