# -*- coding: utf-8 -*-
"""把本地 update/manifest.json 推到 Gitee master 与 main（发版后同步清单）。

API 主机用 api.gitee.com（gitee.com/api/v5/repos 常被 WAF 拦截返回 HTML 403）。

用法：
  python tools/publish_gitee_manifest.py

令牌：仓库根 `.gitee_token`（一行私钥；已 gitignore / 打包排除）。
也可设环境变量 GITEE_ACCESS_TOKEN。
"""
from __future__ import annotations

import base64
import json
import os
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OWNER = "jian0786"
REPO = "starquake"
# master 为发版主分支；main 若存在也必须同步，否则客户端竞速会吃到旧清单。
BRANCHES = ("master", "main")
REL = "update/manifest.json"
CTX = ssl.create_default_context()


def load_token() -> str:
    env = (os.environ.get("GITEE_ACCESS_TOKEN") or "").strip()
    if env:
        return env
    path = ROOT / ".gitee_token"
    if path.is_file():
        tok = path.read_text(encoding="utf-8").strip()
        if tok:
            return tok
    raise SystemExit("缺少令牌：请写仓库根 .gitee_token，或设 GITEE_ACCESS_TOKEN")


def api(method: str, path: str, token: str, data: dict | None = None) -> tuple[int, object]:
    q = urllib.parse.urlencode({"access_token": token})
    url = f"https://api.gitee.com/api/v5{path}"  # gitee.com/api 常被 WAF 403
    url = url + ("&" if "?" in url else "?") + q
    body = None
    headers = {"User-Agent": "StarQuake-publish-gitee-manifest"}
    if data is not None:
        payload = dict(data)
        payload["access_token"] = token
        body = urllib.parse.urlencode(payload).encode("utf-8")
        headers["Content-Type"] = "application/x-www-form-urlencoded"
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=60) as resp:
            raw = resp.read()
            code = resp.status
    except urllib.error.HTTPError as e:
        raw = e.read()
        code = e.code
    if not raw:
        return code, None
    try:
        return code, json.loads(raw.decode("utf-8"))
    except Exception:
        return code, raw.decode("utf-8", "replace")


def put_branch(token: str, branch: str, b64: str) -> tuple[int, str]:
    code, info = api("GET", f"/repos/{OWNER}/{REPO}/contents/{REL}?ref={branch}", token)
    if code == 404:
        # 仓库无该分支则跳过（不要误创文件到错误 ref）
        return 404, f"skip missing branch {branch}"
    sha = info.get("sha") if code == 200 and isinstance(info, dict) else None
    data = {
        "content": b64,
        "message": "chore: sync update/manifest.json",
        "branch": branch,
    }
    if sha:
        data["sha"] = sha
        method = "PUT"
    else:
        # 分支在但文件不存在
        method = "POST"
    code2, out = api(method, f"/repos/{OWNER}/{REPO}/contents/{REL}", token, data)
    if code2 not in (200, 201):
        return code2, f"fail HTTP {code2}: {out}"
    raw_url = f"https://gitee.com/{OWNER}/{REPO}/raw/{branch}/{REL}"
    req = urllib.request.Request(
        raw_url + f"?_={int(__import__('time').time())}",
        headers={
            "User-Agent": "StarQuake-publish-gitee-manifest",
            "Cache-Control": "no-cache",
        },
    )
    with urllib.request.urlopen(req, context=CTX, timeout=30) as resp:
        body = resp.read()
    m = json.loads(body.decode("utf-8"))
    print(f"OK {method} {REL}@{branch}")
    print(f"raw {raw_url}")
    print(f"latest_version {m.get('latest_version')}")
    return 200, "ok"


def main() -> int:
    token = load_token()
    local = ROOT / REL
    if not local.is_file():
        print(f"missing {local}", file=sys.stderr)
        return 1
    content = local.read_bytes()
    b64 = base64.b64encode(content).decode("ascii")

    ok_any = False
    for branch in BRANCHES:
        code, msg = put_branch(token, branch, b64)
        if code == 404:
            print(msg)
            continue
        if code != 200:
            print(msg, file=sys.stderr)
            return 1
        ok_any = True
    if not ok_any:
        print("no branch updated", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
