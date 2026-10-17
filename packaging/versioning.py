#!/usr/bin/env python3
"""Release version grammar shared by every package channel.

A SemVer core without build metadata, optionally followed by one of the
alpha/beta/rc.N (N >= 1) prerelease channels that the release matrix publishes.
Run as a script to validate one version: exit status 0 when it is accepted.
"""
import re
import sys

PRERELEASE_CHANNELS = ("alpha", "beta", "rc")
_NUMBER = r"(?:0|[1-9]\d*)"
RELEASE = re.compile(
    rf"({_NUMBER}\.{_NUMBER}\.{_NUMBER})(?:-({'|'.join(PRERELEASE_CHANNELS)})\.([1-9]\d*))?"
)


def parse_release(version):
    """Return (channel, number) for a release version, or None if invalid."""
    match = RELEASE.fullmatch(version)
    if match is None:
        return None
    _, channel, number = match.groups()
    return (channel, number) if channel else ("stable", None)


if __name__ == "__main__":
    sys.exit(0 if len(sys.argv) == 2 and parse_release(sys.argv[1]) else 1)
