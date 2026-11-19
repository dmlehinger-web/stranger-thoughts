#!/usr/bin/env python3
"""Static checks for the v2 frontend. Fails if a third-party script or a banned banner sneaks in."""

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent
BANNED = [
    "fonts.googleapis.com",
    "fonts.gstatic.com",
    "js.stripe.com",
    "EXAMPLE SCREENS",
    "example totals",
    "onclick=",
    "javascript:",
    "unsafe-inline",
    "sk_live_",
    "sk_test_",
    "whsec_",
]
FONTS = [
    "newsreader-400.woff2",
    "newsreader-400-italic.woff2",
    "newsreader-500.woff2",
    "source-sans-3-400.woff2",
    "source-sans-3-400-italic.woff2",
    "source-sans-3-600.woff2",
]


def main() -> int:
    failures = []
    for font in FONTS:
        path = ROOT / "fonts" / font
        if not path.is_file() or path.stat().st_size < 1000:
            failures.append(f"missing font {font}")
    config = (ROOT / "js" / "config.js").read_text()
    if "mock: true" not in config:
        failures.append("committed config.js must stay in mock mode")
    for path in ROOT.rglob("*"):
        if path.suffix not in {".html", ".js", ".css"}:
            continue
        text = path.read_text()
        for needle in BANNED:
            if needle in text:
                failures.append(f"{path.relative_to(ROOT)} contains {needle}")
        if path.suffix == ".html":
            if "script-src 'self'" not in text:
                failures.append(f"{path.relative_to(ROOT)} is missing the CSP meta tag")
            if 'rel="stylesheet" href="http' in text or "src=\"http" in text:
                failures.append(f"{path.relative_to(ROOT)} loads a remote asset")
    policy = (ROOT.parent / "infra" / "lib" / "headers.ts").read_text()
    for directive in ["default-src 'self'", "script-src 'self'", "object-src 'none'", "frame-ancestors 'none'"]:
        if directive not in policy:
            failures.append(f"CloudFront policy missing {directive}")
    if failures:
        print("\n".join(failures))
        return 1
    print("frontend checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
