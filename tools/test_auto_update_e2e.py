# -*- coding: utf-8 -*-
"""本地自动更新端到端压测（便携 zip + setup 静默安装）。

流程：
1. 单测 update 模块
2. 编译旧版客户端；抬版本编译新版，打 zip +（用户级）setup.exe
3. 场景 A：便携目录 zip 覆盖
4. 场景 B：路径含 Program Files → 优先 setup，/S /D= 安装到 ASCII 目录并替换

用法：
  python tools/test_auto_update_e2e.py
  python tools/test_auto_update_e2e.py --skip-old-build
  python tools/test_auto_update_e2e.py --new-version 2.2.3
  python tools/test_auto_update_e2e.py --only zip|setup
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import zipfile
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parent
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

from _pack_common import workspace_version  # noqa: E402

# ASCII 根：避开中文路径对 NSIS /D= 的坑
E2E_ASCII = Path(os.environ.get("STARQUAKE_E2E_ASCII", r"C:\Users\Huawei\StarQuakeE2E"))


def log(msg: str) -> None:
    print(msg, flush=True)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        while True:
            b = f.read(1024 * 1024)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def set_workspace_version(ver: str) -> str:
    cargo = ROOT / "Cargo.toml"
    text = cargo.read_text(encoding="utf-8")
    old_holder: list[str] = []

    def repl_ws(m: re.Match[str]) -> str:
        block = m.group(0)

        def repl_ver(vm: re.Match[str]) -> str:
            old_holder.append(vm.group(2))
            return f"{vm.group(1)}{ver}{vm.group(3)}"

        new_block, n = re.subn(
            r'(version\s*=\s*")([^"]+)(")',
            repl_ver,
            block,
            count=1,
        )
        if n != 1:
            raise SystemExit("workspace.package 内未找到 version")
        return new_block

    new_text, n = re.subn(
        r"(?ms)^\[workspace\.package\]\n(?:.*?\n)*?(?=\[|\Z)",
        repl_ws,
        text,
        count=1,
    )
    if n != 1:
        raise SystemExit("无法定位 [workspace.package]")
    cargo.write_bytes(new_text.encode("utf-8"))
    return old_holder[0] if old_holder else ""


def cargo_build_release() -> None:
    log("cargo build -p starquake-app --release")
    r = subprocess.run(
        ["cargo", "build", "-p", "starquake-app", "--release"],
        cwd=ROOT,
    )
    if r.returncode != 0:
        raise SystemExit(f"cargo build failed: {r.returncode}")


def rm_tree(path: Path) -> None:
    if not path.exists():
        return
    for _ in range(8):
        try:
            shutil.rmtree(path)
            return
        except OSError:
            time.sleep(0.5)
    raise SystemExit(f"无法删除 {path}")


def assemble_client(dest: Path, exe_src: Path, ver: str, marker: str | None) -> None:
    rm_tree(dest)
    dest.mkdir(parents=True, exist_ok=True)
    shutil.copy2(exe_src, dest / "starquake.exe")
    for name in ("config", "media", "icon", "geodata"):
        src = ROOT / name
        if src.is_dir():
            shutil.copytree(
                src,
                dest / name,
                ignore=shutil.ignore_patterns(".DS_Store", "Thumbs.db", "raw"),
                dirs_exist_ok=False,
            )
    local = dest / "config" / "local.toml"
    if local.is_file():
        local.unlink()
    (dest / "VERSION.txt").write_text(ver + "\n", encoding="utf-8")
    if marker is not None:
        (dest / "update_e2e_marker.txt").write_text(marker, encoding="utf-8")


def pack_zip_from_dir(payload_dir: Path, ver: str) -> Path:
    out_zip = ROOT / "dist" / f"StarQuake_{ver}_portable.zip"
    if out_zip.exists():
        out_zip.unlink()
    n = 0
    with zipfile.ZipFile(out_zip, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=5) as zf:
        for path in payload_dir.rglob("*"):
            if not path.is_file():
                continue
            arc = Path("StarQuake") / path.relative_to(payload_dir)
            zf.write(path, arc.as_posix())
            n += 1
    mb = out_zip.stat().st_size / (1024 * 1024)
    log(f"zip OK files={n} -> {out_zip.name} ({mb:.1f} MB)")
    return out_zip


def find_makensis() -> Path | None:
    which = shutil.which("makensis")
    if which:
        return Path(which)
    for c in (
        ROOT / "target" / "nsis-3.10" / "makensis.exe",
        Path(r"C:\Program Files (x86)\NSIS\makensis.exe"),
        Path(r"C:\Program Files\NSIS\makensis.exe"),
    ):
        if c.is_file():
            return c
    return None


def build_user_setup(payload_dir: Path, ver: str) -> Path:
    """打用户级 setup（无 UAC），供 E2E 静默 /S /D= 覆盖。"""
    makensis = find_makensis()
    if makensis is None:
        log("未找到 makensis，调用 pack_windows_installer.ensure_nsis")
        sys.path.insert(0, str(TOOLS))
        from pack_windows_installer import ensure_nsis  # type: ignore

        makensis = ensure_nsis()

    stage = Path(os.environ.get("TEMP", r"C:\Users\Huawei\AppData\Local\Temp")) / "sq_e2e_nsis"
    rm_tree(stage)
    stage.mkdir(parents=True, exist_ok=True)
    payload = stage / "payload"
    shutil.copytree(payload_dir, payload)
    out = ROOT / "dist" / f"StarQuake_{ver}_setup.exe"
    stage_out = stage / f"StarQuake_{ver}_setup.exe"
    nsi = stage / "e2e_user.nsi"
    # NSIS 路径用正斜杠更稳
    payload_nsis = str(payload).replace("\\", "/")
    out_nsis = str(stage_out).replace("\\", "/")
    nsi.write_text(
        f"""Unicode true
RequestExecutionLevel user
Name "StarQuake"
OutFile "{out_nsis}"
InstallDir "$LOCALAPPDATA\\Programs\\StarQuake"
SetCompressor /SOLID lzma

Section "StarQuake"
  SetOutPath "$INSTDIR"
  File /r "{payload_nsis}\\*.*"
SectionEnd
""",
        encoding="utf-8",
    )
    cmd = [str(makensis), str(nsi)]
    log(" ".join(cmd))
    r = subprocess.run(cmd, cwd=str(stage))
    if r.returncode != 0:
        raise SystemExit(f"makensis failed: {r.returncode}")
    if not stage_out.is_file():
        raise SystemExit(f"setup not produced: {stage_out}")
    shutil.copy2(stage_out, out)
    log(f"setup OK -> {out.name} ({out.stat().st_size / 1024 / 1024:.1f} MB)")
    return out


class _QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, fmt: str, *args) -> None:  # noqa: A003
        log(f"[http] {args[0] if args else fmt}")


def start_http(directory: Path, port: int = 0) -> tuple[ThreadingHTTPServer, int]:
    def factory(*a, **k):
        return _QuietHandler(*a, directory=str(directory), **k)

    httpd = ThreadingHTTPServer(("127.0.0.1", port), factory)
    actual = httpd.server_address[1]
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd, actual


def kill_starquake_under(*roots: Path) -> None:
    roots_l = [str(r.resolve()).lower() for r in roots if r]
    try:
        out = subprocess.check_output(
            [
                "powershell",
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_Process -Filter \"name='starquake.exe'\" | "
                "Select-Object ProcessId,ExecutablePath | ConvertTo-Json -Compress",
            ],
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except Exception:
        return
    out = (out or "").strip()
    if not out:
        return
    try:
        data = json.loads(out)
    except json.JSONDecodeError:
        return
    if isinstance(data, dict):
        data = [data]
    for row in data or []:
        path = (row.get("ExecutablePath") or "").replace("/", "\\").lower()
        if any(path.startswith(r) for r in roots_l):
            pid = row.get("ProcessId")
            if pid:
                subprocess.run(
                    ["taskkill", "/PID", str(pid), "/F"],
                    capture_output=True,
                )


def wait_marker(target: Path, new_ver: str, proc: subprocess.Popen, label: str, timeout: float = 600) -> bool:
    marker = target / "update_e2e_marker.txt"
    ver_file = target / "VERSION.txt"
    started = time.time()
    last = 0.0
    while time.time() - started < timeout:
        if marker.is_file() and ver_file.is_file():
            body = marker.read_text(encoding="utf-8").strip()
            ver_txt = ver_file.read_text(encoding="utf-8").strip()
            if body == new_ver and ver_txt == new_ver:
                return True
        now = time.time()
        if now - last > 30:
            log(
                f"[{label}] waiting... marker={marker.is_file()} "
                f"proc={proc.poll()} elapsed={int(now - started)}s"
            )
            last = now
        time.sleep(1.5)
    return False


def run_scenario(
    *,
    label: str,
    client_dir: Path,
    old_exe: Path,
    old_ver: str,
    new_ver: str,
    man_url: str,
    expect_http_substr: str,
    extra_env: dict[str, str],
    watch_dirs: list[Path],
    marker_dir: Path,
    log_path: Path,
) -> bool:
    kill_starquake_under(*watch_dirs)
    assemble_client(client_dir, old_exe, old_ver, marker=None)
    # 清理目标 marker（安装场景 marker 在 install 目录）
    m = marker_dir / "update_e2e_marker.txt"
    if m.exists():
        m.unlink()
    if marker_dir != client_dir:
        # 安装目标先放旧版壳，验证被 setup 覆盖
        if marker_dir.exists():
            # 保留目录结构但去掉 marker
            vf = marker_dir / "VERSION.txt"
            if vf.is_file():
                vf.write_text(old_ver + "\n", encoding="utf-8")
        else:
            marker_dir.mkdir(parents=True, exist_ok=True)
            (marker_dir / "VERSION.txt").write_text(old_ver + "\n", encoding="utf-8")

    env = os.environ.copy()
    env["STARQUAKE_UPDATE_E2E"] = "1"
    env["STARQUAKE_UPDATE_MANIFEST_URL"] = man_url
    env["RUST_LOG"] = "info"
    env.pop("STARQUAKE_AUTOTEST", None)
    env.update(extra_env)

    log(f"=== [{label}] launch {client_dir / 'starquake.exe'} ===")
    with log_path.open("w", encoding="utf-8") as lf:
        proc = subprocess.Popen(
            [str(client_dir / "starquake.exe")],
            cwd=str(client_dir),
            env=env,
            stdout=lf,
            stderr=subprocess.STDOUT,
        )
    ok = wait_marker(marker_dir, new_ver, proc, label)
    kill_starquake_under(*watch_dirs)
    if not ok:
        log(f"FAIL [{label}]")
        if log_path.is_file():
            log(log_path.read_text(encoding="utf-8", errors="replace")[-5000:])
        return False
    # 粗检 http 访问日志由调用方看 terminal；此处确认产物
    log(f"PASS [{label}] -> {marker_dir}")
    return True


def main() -> int:
    args = sys.argv[1:]
    skip_old = "--skip-old-build" in args
    new_ver = "2.2.3"
    if "--new-version" in args:
        new_ver = args[args.index("--new-version") + 1]
    only = None
    if "--only" in args:
        only = args[args.index("--only") + 1].strip().lower()

    old_ver = workspace_version(ROOT)
    if old_ver == new_ver:
        log(f"当前版本已是 {new_ver}，请指定更高 --new-version")
        return 1

    log(f"=== 自动更新全量 E2E：{old_ver} → {new_ver} ===")
    log("预估约 15–25 分钟（两次 release + zip + setup + 两场景）")

    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    e2e_new = dist / "e2e_new_payload"
    old_stash = dist / "e2e_old_exe" / "starquake.exe"
    exe_src = ROOT / "target" / "release" / "starquake.exe"

    log("cargo test -p starquake-app -- update::tests --test-threads=1")
    r = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "starquake-app",
            "--",
            "update::tests",
            "--test-threads=1",
        ],
        cwd=ROOT,
    )
    if r.returncode != 0:
        return r.returncode

    if not skip_old or not old_stash.is_file():
        if workspace_version(ROOT) != old_ver:
            set_workspace_version(old_ver)
        cargo_build_release()
        old_stash.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(exe_src, old_stash)
        log(f"stashed old exe -> {old_stash}")
    else:
        log("skip old build, use stashed exe")

    prev = set_workspace_version(new_ver)
    log(f"bumped version {prev} -> {new_ver}")
    zip_path = setup_path = None
    try:
        cargo_build_release()
        assemble_client(e2e_new, exe_src, new_ver, marker=new_ver)
        zip_path = pack_zip_from_dir(e2e_new, new_ver)
        setup_path = build_user_setup(e2e_new, new_ver)
    finally:
        set_workspace_version(old_ver)
        log(f"restored Cargo.toml version -> {old_ver}")

    assert zip_path and setup_path
    zip_sha = sha256_file(zip_path)
    setup_sha = sha256_file(setup_path)

    httpd, port = start_http(dist, 0)
    base = f"http://127.0.0.1:{port}"
    results: list[tuple[str, bool]] = []

    try:
        # ---- A: portable zip ----
        if only in (None, "zip"):
            man = {
                "latest_version": new_ver,
                "min_supported_version": "2.0.0",
                "channel": "stable",
                "notes": f"E2E zip {old_ver}->{new_ver}",
                "zip": {"url": f"{base}/{zip_path.name}", "sha256": zip_sha},
                "installer": {
                    "url": f"{base}/{setup_path.name}",
                    "sha256": setup_sha,
                },
                "html_url": base + "/",
            }
            man_path = dist / "e2e_manifest_zip.json"
            man_path.write_text(
                json.dumps(man, ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )
            client = dist / "e2e_client_zip"
            ok = run_scenario(
                label="zip",
                client_dir=client,
                old_exe=old_stash,
                old_ver=old_ver,
                new_ver=new_ver,
                man_url=f"{base}/e2e_manifest_zip.json",
                expect_http_substr=zip_path.name,
                extra_env={"STARQUAKE_UPDATE_PREFER_PORTABLE": "1"},
                watch_dirs=[client],
                marker_dir=client,
                log_path=dist / "e2e_update_zip.log",
            )
            results.append(("zip", ok))

        # ---- B: setup silent replace ----
        if only in (None, "setup"):
            E2E_ASCII.mkdir(parents=True, exist_ok=True)
            # 路径含 "Program Files" → 安装包通道；并设 PREFER_INSTALLER 双保险
            client = E2E_ASCII / "Program Files" / "StarQuake"
            install_dir = E2E_ASCII / "install"
            rm_tree(install_dir)
            # 预置旧壳，确认被替换
            assemble_client(install_dir, old_stash, old_ver, marker=None)
            (install_dir / "update_e2e_marker.txt").unlink(missing_ok=True)

            man = {
                "latest_version": new_ver,
                "min_supported_version": "2.0.0",
                "channel": "stable",
                "notes": f"E2E setup {old_ver}->{new_ver}",
                "zip": {"url": f"{base}/{zip_path.name}", "sha256": zip_sha},
                "installer": {
                    "url": f"{base}/{setup_path.name}",
                    "sha256": setup_sha,
                },
                "html_url": base + "/",
            }
            man_path = dist / "e2e_manifest_setup.json"
            man_path.write_text(
                json.dumps(man, ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )
            ok = run_scenario(
                label="setup",
                client_dir=client,
                old_exe=old_stash,
                old_ver=old_ver,
                new_ver=new_ver,
                man_url=f"{base}/e2e_manifest_setup.json",
                expect_http_substr=setup_path.name,
                extra_env={
                    "STARQUAKE_UPDATE_PREFER_INSTALLER": "1",
                    "STARQUAKE_UPDATE_INSTALL_DIR": str(install_dir),
                },
                watch_dirs=[client, install_dir, E2E_ASCII],
                marker_dir=install_dir,
                log_path=dist / "e2e_update_setup.log",
            )
            results.append(("setup", ok))
    finally:
        kill_starquake_under(dist / "e2e_client_zip", E2E_ASCII)
        try:
            httpd.shutdown()
        except Exception:
            pass

    log("=== 汇总 ===")
    all_ok = True
    for name, ok in results:
        log(f"  {name}: {'PASS' if ok else 'FAIL'}")
        all_ok = all_ok and ok
    if not results:
        log("未运行任何场景")
        return 1
    return 0 if all_ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
