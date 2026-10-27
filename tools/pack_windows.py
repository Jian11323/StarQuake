# -*- coding: utf-8 -*-
"""Windows 便携包：release 构建 + 组装 + zip（纯 Python，避免 PS 卡死）。"""
from __future__ import annotations

import shutil
import subprocess
import sys
import time
import zipfile
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parent
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

from _pack_common import (  # noqa: E402
    assert_brand_assets,
    copy_ignore_patterns,
    display_version,
    pack_version_slug,
)


def log(msg: str) -> None:
    print(msg, flush=True)


def rm_tree(path: Path) -> None:
    if not path.exists():
        return
    for attempt in range(8):
        try:
            shutil.rmtree(path)
            return
        except OSError as e:
            log(f"remove retry {attempt + 1}: {e}")
            time.sleep(0.6)
    bak = path.with_name(path.name + f".old_{int(time.time())}")
    try:
        path.rename(bak)
        log(f"renamed locked dir -> {bak}")
    except OSError as e:
        raise SystemExit(f"无法清理输出目录 {path}: {e}") from e


def copy_tree(src: Path, dst: Path) -> None:
    log(f"copy {src.name} ...")
    shutil.copytree(
        src,
        dst,
        ignore=shutil.ignore_patterns(*copy_ignore_patterns()),
        dirs_exist_ok=False,
    )


def write_usage(out: Path, ver_display: str) -> None:
    usage = f"""StarQuake 便携包
================

版本：{ver_display}
双击 starquake.exe 即可运行（勿拆散 media / icon / geodata / config 相对位置）。
启动时会显示启动界面，底图就绪后进入主界面。

【首次使用】
1. 打开后点「设置」→ API与数据源，用 lk_ 换票绑定 Jian（可选）。
2. 未配置 Jian 时，Wolfx / P2P / Pancakes 仍可用；Jian 健康态为「异常」。
3. 用户配置写到本机 AppData，不回写本目录。
4. 运行日志：%APPDATA%\StarQuake\log.txt；本地配置：同目录 settings.json。设置 → 高级可打开。

【版本规则】
- PATCH（x.y.Z）：修 bug / 微调，无新能力
- MINOR（x.Y.0）：新功能或行为变更
- MAJOR（X.0.0）：不兼容变更
"""
    (out / "使用说明.txt").write_bytes(usage.encode("utf-8-sig"))


def zip_out(out: Path, ver_slug: str) -> Path:
    zip_path = ROOT / "dist" / f"StarQuake_{ver_slug}_portable.zip"
    if zip_path.exists():
        zip_path.unlink()
    skip = {".DS_Store", "Thumbs.db"}
    n = 0
    log(f"zip -> {zip_path.name} ...")
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=5) as zf:
        for path in out.rglob("*"):
            if not path.is_file() or path.name in skip:
                continue
            # 跳过 travel/raw（若误拷入）
            parts = set(path.parts)
            if "raw" in parts and "travel" in parts:
                continue
            arc = Path("StarQuake") / path.relative_to(out)
            zf.write(path, arc.as_posix())
            n += 1
            if n % 100 == 0:
                log(f"  zipped {n} files...")
    mb = zip_path.stat().st_size / (1024 * 1024)
    log(f"zip OK files={n} size={mb:.1f} MB")
    return zip_path


def main() -> int:
    ver_display = display_version(ROOT)
    ver_slug = pack_version_slug(ROOT)
    log(f"StarQuake {ver_display} portable pack")
    assert_brand_assets(ROOT)

    skip_build = "--skip-build" in sys.argv
    exe_src = ROOT / "target" / "release" / "starquake.exe"
    if not skip_build or not exe_src.is_file():
        log("cargo build -p starquake-app --release")
        r = subprocess.run(
            ["cargo", "build", "-p", "starquake-app", "--release"],
            cwd=ROOT,
        )
        if r.returncode != 0:
            return r.returncode
    else:
        log("skip build, use existing release exe")

    if not exe_src.is_file():
        log(f"missing {exe_src}")
        return 1

    out = ROOT / "dist" / "StarQuake"
    log(f"clean {out}")
    rm_tree(out)
    out.mkdir(parents=True, exist_ok=True)

    shutil.copy2(exe_src, out / "starquake.exe")
    for name in ("README.md", "LICENSE"):
        src = ROOT / name
        if src.is_file():
            shutil.copy2(src, out / name)

    copy_tree(ROOT / "config", out / "config")
    local = out / "config" / "local.toml"
    if local.is_file():
        local.unlink()

    copy_tree(ROOT / "media", out / "media")
    copy_tree(ROOT / "icon", out / "icon")
    raw = out / "media" / "travel" / "raw"
    if raw.is_dir():
        shutil.rmtree(raw)

    copy_tree(ROOT / "geodata", out / "geodata")

    write_usage(out, ver_display)
    zpath = zip_out(out, ver_slug)
    log(f"DIR  {out}")
    log(f"ZIP  {zpath}")
    log(f"RUN  {out / 'starquake.exe'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
