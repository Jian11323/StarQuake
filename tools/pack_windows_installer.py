# -*- coding: utf-8 -*-
"""Windows 安装包：基于已组装的 dist/StarQuake + NSIS → setup.exe。"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parent
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

from _pack_common import assert_brand_assets, display_version, pack_version_slug  # noqa: E402

NSIS_URL = "https://sourceforge.net/projects/nsis/files/NSIS%203/3.10/nsis-3.10.zip/download"
NSIS_CACHE = ROOT / "target" / "nsis-3.10"


def log(msg: str) -> None:
    print(msg, flush=True)


def find_makensis() -> Path | None:
    which = shutil.which("makensis")
    if which:
        return Path(which)
    for c in (
        NSIS_CACHE / "makensis.exe",
        Path(r"C:\Program Files (x86)\NSIS\makensis.exe"),
        Path(r"C:\Program Files\NSIS\makensis.exe"),
    ):
        if c.is_file():
            return c
    return None


def ensure_nsis() -> Path:
    found = find_makensis()
    if found:
        return found
    NSIS_CACHE.parent.mkdir(parents=True, exist_ok=True)
    zip_path = ROOT / "target" / "nsis-3.10.zip"
    if not zip_path.is_file():
        log(f"download NSIS -> {zip_path.name}")
        req = urllib.request.Request(
            NSIS_URL,
            headers={"User-Agent": "StarQuake-packager"},
        )
        with urllib.request.urlopen(req, timeout=180) as resp, zip_path.open("wb") as f:
            shutil.copyfileobj(resp, f)
    log("extract NSIS ...")
    if NSIS_CACHE.exists():
        shutil.rmtree(NSIS_CACHE)
    with zipfile.ZipFile(zip_path) as zf:
        zf.extractall(ROOT / "target")
    extracted = ROOT / "target" / "nsis-3.10"
    if not extracted.is_dir():
        for p in (ROOT / "target").iterdir():
            if p.is_dir() and (p / "makensis.exe").is_file():
                extracted = p
                break
    if extracted.resolve() != NSIS_CACHE.resolve():
        if NSIS_CACHE.exists():
            shutil.rmtree(NSIS_CACHE)
        extracted.rename(NSIS_CACHE)
    exe = NSIS_CACHE / "makensis.exe"
    if not exe.is_file():
        raise SystemExit(f"makensis not found after extract: {NSIS_CACHE}")
    return exe


def ensure_portable(*, force: bool) -> Path:
    dist = ROOT / "dist" / "StarQuake"
    exe = dist / "starquake.exe"
    splash = dist / "icon" / "brand" / "splash_bg.png"
    need = force or (not exe.is_file()) or (not splash.is_file()) or (not (dist / "media").is_dir() and (dist / "icon").is_dir())
    if not need:
        return dist
    log("组装便携目录 → run pack_windows.py")
    r = subprocess.run([sys.executable, str(ROOT / "tools" / "pack_windows.py")], cwd=ROOT)
    if r.returncode != 0:
        raise SystemExit(r.returncode)
    if not exe.is_file():
        raise SystemExit(f"missing {exe}")
    if not splash.is_file():
        raise SystemExit(f"missing splash: {splash}")
    return dist


def main() -> int:
    force = "--force-portable" in sys.argv or "--rebuild" in sys.argv
    # 默认强制重建，避免沿用旧 1.1.0 目录漏掉启动界面等新资源
    if "--reuse-portable" not in sys.argv:
        force = True

    ver_display = display_version(ROOT)
    ver_slug = pack_version_slug(ROOT)
    log(f"StarQuake {ver_display} Windows installer")
    assert_brand_assets(ROOT)
    dist = ensure_portable(force=force)
    makensis = ensure_nsis()
    out = ROOT / "dist" / f"StarQuake_{ver_slug}_setup.exe"
    if out.exists():
        out.unlink()

    # ASCII staging paths — avoid NSIS/cmdline Unicode path issues
    stage_root = Path(os.environ.get("TEMP", tempfile.gettempdir())) / "starquake_nsis"
    stage_dist = stage_root / "StarQuake"
    stage_out = stage_root / f"StarQuake_{ver_slug}_setup.exe"
    if stage_root.exists():
        shutil.rmtree(stage_root, ignore_errors=True)
    stage_root.mkdir(parents=True, exist_ok=True)
    log(f"stage copy -> {stage_dist}")
    shutil.copytree(dist, stage_dist)

    # NSIS MUI 欢迎页侧栏需要 BMP；由启动图生成
    splash_png = stage_dist / "icon" / "brand" / "splash_bg.png"
    splash_bmp = stage_dist / "icon" / "brand" / "splash_installer.bmp"
    if splash_png.is_file():
        try:
            from PIL import Image  # type: ignore
        except ImportError:
            subprocess.check_call([sys.executable, "-m", "pip", "install", "pillow", "-q"])
            from PIL import Image  # type: ignore
        im = Image.open(splash_png).convert("RGB")
        w, h = 164, 314
        im_ratio = im.width / max(im.height, 1)
        target_ratio = w / h
        if im_ratio > target_ratio:
            nh, nw = h, int(h * im_ratio)
        else:
            nw, nh = w, int(w / max(im_ratio, 1e-6))
        im = im.resize((nw, nh), Image.Resampling.LANCZOS)
        left, top = (nw - w) // 2, (nh - h) // 2
        im.crop((left, top, left + w, top + h)).save(splash_bmp, format="BMP")
        log(f"installer splash bmp -> {splash_bmp.name}")
    else:
        log(f"WARN missing splash png: {splash_png}")


    nsi = ROOT / "tools" / "pack_windows_installer.nsi"
    cmd = [
        str(makensis),
        f"/DVERSION={ver_slug}",
        f"/DDISPLAY_VERSION={ver_display}",
        f"/DDIST_DIR={stage_dist}",
        f"/DOUT_FILE={stage_out}",
        f"/DSPLASH_BMP={splash_bmp}",
        str(nsi),
    ]
    log(" ".join(cmd))
    r = subprocess.run(cmd, cwd=str(ROOT / "tools"))
    if r.returncode != 0:
        return r.returncode
    if not stage_out.is_file():
        raise SystemExit(f"installer not produced: {stage_out}")
    shutil.copy2(stage_out, out)
    mb = out.stat().st_size / (1024 * 1024)
    log(f"SETUP {out} ({mb:.1f} MB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
