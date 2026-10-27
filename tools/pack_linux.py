# -*- coding: utf-8 -*-
"""Linux x86_64 便携包：组装目录并打 tar.gz。"""
from __future__ import annotations

import os
import platform
import shutil
import subprocess
import sys
import tarfile
import time
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

LINUX_TARGET = "x86_64-unknown-linux-gnu"
# glibc 2.31+（Ubuntu 20.04 / Debian 11 一类）
ZIG_TARGET = "x86_64-unknown-linux-gnu.2.31"


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
            time.sleep(0.5)
    raise SystemExit(f"无法清理 {path}")


def copy_tree(src: Path, dst: Path) -> None:
    log(f"copy {src.name} ...")
    shutil.copytree(
        src,
        dst,
        ignore=shutil.ignore_patterns(*copy_ignore_patterns()),
    )


def write_usage(out: Path, ver: str) -> None:
    usage = f"""StarQuake 便携包（Linux x86_64）
================================

版本：{ver}
解压后：chmod +x starquake && ./starquake
（勿拆散 media / icon / geodata / config）
启动时会显示启动界面，底图就绪后进入主界面。

【依赖】Debian/Ubuntu 示例：
  sudo apt install libasound2 libxcb-render0 libxcb-shape0 libxcb-xfixes0 \\
    libxkbcommon0 libgtk-3-0

【首次使用】
1. 设置 → API与数据源，用 lk_ 换票绑定 Jian（可选）
2. 未配置 Jian 时 Wolfx / P2P / Pancakes 仍可用
3. 配置写在用户目录，不回写本包

【版本规则】
- PATCH：修 bug / 微调
- MINOR：新功能或行为变更
- MAJOR：不兼容变更
"""
    (out / "使用说明.txt").write_bytes(usage.encode("utf-8"))
    run_sh = out / "run.sh"
    run_sh.write_text(
        '#!/bin/sh\ncd "$(dirname "$0")"\nexec ./starquake "$@"\n',
        encoding="utf-8",
        newline="\n",
    )
    try:
        run_sh.chmod(0o755)
    except OSError:
        pass


def find_bin(cli_bin: str | None) -> Path:
    if cli_bin:
        p = Path(cli_bin)
        if not p.is_file():
            raise SystemExit(f"binary not found: {p}")
        return p
    for c in (
        ROOT / "target" / LINUX_TARGET / "release" / "starquake",
        ROOT / "target" / "release" / "starquake",
    ):
        if c.is_file():
            return c
    raise SystemExit(
        "未找到 Linux 二进制。本机 Linux/WSL: cargo build -p starquake-app --release\n"
        f"Windows 交叉: cargo zigbuild -p starquake-app --release --target {ZIG_TARGET}\n"
        "然后: python tools/pack_linux.py --skip-build --bin <path>"
    )



def try_wsl_build() -> int:
    """Windows：在 StarQuakeLinux（或 STARQUAKE_WSL_DISTRO）里原生编译。"""
    distro = os.environ.get("STARQUAKE_WSL_DISTRO", "StarQuakeLinux")
    rest = ROOT.as_posix()
    if len(rest) >= 2 and rest[1] == ":":
        linux_root = "/mnt/" + rest[0].lower() + rest[2:]
    else:
        linux_root = rest
    # 写到纯 ASCII 路径，避免中文路径经登录 shell / 控制台编码弄坏
    script = Path(r"D:\tools\_sq_wsl_build.sh")
    script.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        "#!/bin/bash",
        "set -e",
        "export RUSTUP_HOME=/root/.rustup",
        "export CARGO_HOME=/root/.cargo",
        'export PATH="$CARGO_HOME/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"',
        '. "$CARGO_HOME/env"',
        f'cd "{linux_root}"',
        "cargo build -p starquake-app --release",
        "",
    ]
    script.write_bytes("\n".join(lines).encode("utf-8"))
    cmd = [
        "wsl",
        "-d",
        distro,
        "-u",
        "root",
        "--",
        "bash",
        "/mnt/d/tools/_sq_wsl_build.sh",
    ]
    log(" ".join(cmd))
    return subprocess.run(cmd).returncode


def try_zigbuild() -> int:
    """Windows 上优先 cargo zigbuild；失败返回非 0。"""
    cmd = [
        "cargo",
        "zigbuild",
        "-p",
        "starquake-app",
        "--release",
        "--target",
        ZIG_TARGET,
    ]
    log(" ".join(cmd))
    return subprocess.run(cmd, cwd=ROOT).returncode


def main() -> int:
    args = sys.argv[1:]
    skip_build = "--skip-build" in args
    cli_bin = None
    if "--bin" in args:
        cli_bin = args[args.index("--bin") + 1]

    ver_display = display_version(ROOT)
    ver_slug = pack_version_slug(ROOT)
    log(f"StarQuake {ver_display} Linux x86_64 portable")
    assert_brand_assets(ROOT)

    if not skip_build and cli_bin is None:
        if platform.system() == "Linux":
            log("cargo build -p starquake-app --release")
            r = subprocess.run(
                ["cargo", "build", "-p", "starquake-app", "--release"], cwd=ROOT
            )
            if r.returncode != 0:
                return r.returncode
        else:
            # Windows：优先 WSL（与 CI 一致）；无 WSL 再试 zigbuild
            if try_wsl_build() == 0:
                pass
            else:
                log("WSL 编译失败，尝试 cargo zigbuild…")
                if try_zigbuild() != 0:
                    raise SystemExit(
                        "Linux 编译失败。请先：python tools/setup_linux_pack_env.py --wsl\n"
                        "或安装 Zig 后重试 cargo zigbuild，或手动 --bin"
                    )

    bin_src = find_bin(cli_bin)
    log(f"binary: {bin_src}")

    out = ROOT / "dist" / "StarQuake-linux"
    rm_tree(out)
    out.mkdir(parents=True)

    dst = out / "starquake"
    shutil.copy2(bin_src, dst)
    try:
        dst.chmod(0o755)
    except OSError:
        pass

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

    tar_path = ROOT / "dist" / f"StarQuake_{ver_slug}_linux_x86_64.tar.gz"
    if tar_path.exists():
        tar_path.unlink()
    log(f"tar -> {tar_path.name}")
    with tarfile.open(tar_path, "w:gz") as tf:
        tf.add(out, arcname="StarQuake")
    mb = tar_path.stat().st_size / (1024 * 1024)
    log(f"DIR {out}")
    log(f"TAR {tar_path} ({mb:.1f} MB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
