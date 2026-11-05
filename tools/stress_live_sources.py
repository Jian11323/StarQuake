# -*- coding: utf-8 -*-
"""全数据源真实上游压测（可选）。

默认跳过。开启：
  set EEWVIEW_STRESS=1
  python tools/stress_live_sources.py

全功能（忽略配置里部分关闭的子源，尽量全开）:
  set EEWVIEW_STRESS_FULL=1

环境变量：
  EEWVIEW_STRESS_SECS   运行秒数，默认 180（FULL 时默认 300）
  EEWVIEW_STRESS_WS     1=WebSocket（需 websockets），默认 1

覆盖：config/default.toml + config/local.toml +
  %APPDATA%/StarQuake/settings.json（仅读取 at_/rt_，密钥不入库、不写入报告）。
报告：logs/stress_live_YYYYMMDD_HHMMSS.json
"""
from __future__ import annotations

import json
import os
import re
import sys
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]

# —— 默认端点（与 starquake-config 默认一致）——
DEFAULTS = {
    "wolfx_eqlist": "https://api.wolfx.jp/cenc_eqlist.json",
    "wolfx_ws": "wss://ws-api.wolfx.jp/all_eew",
    "p2p_http": "https://api.p2pquake.net/v2/history",
    "p2p_ws": "wss://api.p2pquake.net/v2/ws",
    "jian_ws": "wss://api.sismotide.top/all",
    "kmoni_ws": "wss://api.sismotide.top/kmoni",
    "snet_ws": "wss://api.sismotide.top/s-net",
    "kma_ws": "wss://api.sismotide.top/kma-station",
    "pancakes_base": "https://api.aloys23.link",
    "pancakes_ws": "wss://api.aloys23.link",
    "typhoon": "https://api.sismotide.top/get/typhoon.json",
    "radar_cn": "https://api.sismotide.top/get/radar.json",
    "radar_jp": "https://api.sismotide.top/get/radar-japan.json",
    "radar_tw": "https://api.sismotide.top/get/radar-taiwan.json",
    "precip": "https://api.sismotide.top/get/precip-forecast.json",
    "weather_ws": "wss://api.sismotide.top/weather",
}

UA = "StarQuake-stress/2.0 (+https://github.com/Jian11323/StarQuake)"


def _truthy(v: str | None) -> bool:
    return (v or "").strip().lower() in ("1", "true", "yes", "on")


def load_merged_toml_text() -> str:
    parts: list[str] = []
    for name in ("default.toml", "local.toml"):
        p = ROOT / "config" / name
        if p.is_file():
            parts.append(p.read_text(encoding="utf-8"))
    return "\n".join(parts)


def parse_cfg(text: str) -> dict[str, Any]:
    """粗解析 TOML 开关与 URL（不依赖 tomllib，避免密钥进报告）。"""
    cfg: dict[str, Any] = {
        "jian_enabled": True,
        "wolfx_enabled": True,
        "p2p_enabled": True,
        "pancakes_enabled": True,
        "typhoon_enabled": True,
        "map_wx_enabled": True,
        "weather_enabled": True,
        "jma_station": True,
        "snet": False,
        "kma_station": True,
        "urls": dict(DEFAULTS),
        "has_jian_at": False,
        "has_jian_rt": False,
        "jian_at": "",
        "jian_rt": "",
    }
    section = ""
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip()
            continue
        if "=" not in line:
            continue
        key, _, val = line.partition("=")
        key = key.strip()
        val = val.split("#", 1)[0].strip()

        def unquote(s: str) -> str:
            s = s.strip()
            if len(s) >= 2 and s[0] == s[-1] and s[0] in "\"'":
                return s[1:-1]
            return s

        if key == "enabled":
            on = unquote(val).lower() in ("true", "1", "yes")
            if section == "sources.jian":
                cfg["jian_enabled"] = on
            elif section == "sources.wolfx":
                cfg["wolfx_enabled"] = on
            elif section == "sources.p2pquake":
                cfg["p2p_enabled"] = on
            elif section == "sources.pancakes":
                cfg["pancakes_enabled"] = on
            elif section == "sources.typhoon":
                cfg["typhoon_enabled"] = on
            elif section == "sources.map_wx":
                cfg["map_wx_enabled"] = on
            elif section == "sources.weather":
                cfg["weather_enabled"] = on

        if section == "sources.stations":
            if key == "jma_enabled":
                cfg["jma_station"] = unquote(val).lower() in ("true", "1", "yes")
            if key == "snet_enabled":
                cfg["snet"] = unquote(val).lower() in ("true", "1", "yes")
            if key == "kma_enabled":
                cfg["kma_station"] = unquote(val).lower() in ("true", "1", "yes")
            if key == "jma_jian_ws":
                cfg["urls"]["kmoni_ws"] = unquote(val)
            if key == "snet_jian_ws":
                cfg["urls"]["snet_ws"] = unquote(val)
            if key == "kma_jian_ws":
                cfg["urls"]["kma_ws"] = unquote(val)

        if section == "sources.jian":
            if key == "ws_url":
                cfg["urls"]["jian_ws"] = unquote(val)
            if key == "access_token":
                tok = unquote(val).strip()
                if tok.startswith("at_"):
                    cfg["has_jian_at"] = True
                    cfg["jian_at"] = tok
            if key == "refresh_token":
                tok = unquote(val).strip()
                if tok.startswith("rt_"):
                    cfg["has_jian_rt"] = True
                    cfg["jian_rt"] = tok

        if section == "sources.wolfx":
            if key == "http_eqlist":
                cfg["urls"]["wolfx_eqlist"] = unquote(val)
            if key == "ws_urls":
                m = re.search(r'"([^"]+)"', val)
                if m:
                    cfg["urls"]["wolfx_ws"] = m.group(1)

        if section == "sources.p2pquake":
            if key == "http_history":
                cfg["urls"]["p2p_http"] = unquote(val)
            if key == "ws_url":
                cfg["urls"]["p2p_ws"] = unquote(val)

        if section == "sources.pancakes":
            if key == "base_url":
                cfg["urls"]["pancakes_base"] = unquote(val).rstrip("/")
            if key == "ws_url":
                cfg["urls"]["pancakes_ws"] = unquote(val)

        if section == "sources.typhoon" and key == "url":
            cfg["urls"]["typhoon"] = unquote(val)

        if section == "sources.map_wx":
            if key == "radar_china_url":
                cfg["urls"]["radar_cn"] = unquote(val)
            if key == "radar_japan_url":
                cfg["urls"]["radar_jp"] = unquote(val)
            if key == "radar_taiwan_url":
                cfg["urls"]["radar_tw"] = unquote(val)
            if key == "precip_url":
                cfg["urls"]["precip"] = unquote(val)

        if section == "sources.weather" and key == "ws_url":
            cfg["urls"]["weather_ws"] = unquote(val)

    return cfg


def merge_user_settings_tokens(cfg: dict[str, Any]) -> None:
    """从用户目录 settings.json 补 at_/rt_（不写进报告正文）。"""
    appdata = os.environ.get("APPDATA") or ""
    if not appdata:
        return
    path = Path(appdata) / "StarQuake" / "settings.json"
    if not path.is_file():
        return
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return
    jian = (data.get("sources") or {}).get("jian") or {}
    at = str(jian.get("access_token") or "").strip()
    rt = str(jian.get("refresh_token") or "").strip()
    if at.startswith("at_"):
        cfg["has_jian_at"] = True
        cfg["jian_at"] = at
    if rt.startswith("rt_"):
        cfg["has_jian_rt"] = True
        cfg["jian_rt"] = rt


def exchange_rt_to_at(refresh_token: str) -> str | None:
    """rt_ → at_（对齐客户端 auth::exchange_access_token）。"""
    rt = (refresh_token or "").strip()
    if not rt.startswith("rt_"):
        return None
    req = urllib.request.Request(
        "https://auth.sismotide.top/api/access",
        data=b"",
        method="POST",
        headers={
            "User-Agent": UA,
            "Authorization": f"Bearer {rt}",
            "Accept": "application/json",
            "Content-Length": "0",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read()
        data = json.loads(raw.decode("utf-8", errors="replace"))
        tok = str(
            (data.get("token") if isinstance(data, dict) else None)
            or (data.get("Data", {}) if isinstance(data, dict) else {}).get("token")
            or ""
        ).strip()
        if tok.startswith("at_"):
            return tok
        # 兼容字段名 access_token
        tok2 = str((data.get("access_token") if isinstance(data, dict) else None) or "").strip()
        return tok2 if tok2.startswith("at_") else None
    except Exception as e:  # noqa: BLE001
        print(f"  warn: rt_->at_ exchange failed: {e}")
        return None


def jian_query_key(cfg: dict[str, Any]) -> str | None:
    """鉴权必须用短期 at_；仅有 rt_ 时先换票。"""
    if cfg.get("jian_at") and str(cfg["jian_at"]).startswith("at_"):
        return cfg["jian_at"]
    if cfg.get("jian_rt"):
        at = exchange_rt_to_at(cfg["jian_rt"])
        if at:
            cfg["jian_at"] = at
            cfg["has_jian_at"] = True
            print("  jian: exchanged rt_ -> at_ for WS key")
            return at
    return None


def http_once(url: str, timeout: float = 25.0) -> dict[str, Any]:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read()
            ms = (time.perf_counter() - t0) * 1000
            parse_ok = False
            try:
                data = json.loads(raw.decode("utf-8", errors="replace"))
                parse_ok = isinstance(data, (dict, list))
            except json.JSONDecodeError:
                # 部分接口可能非 JSON
                parse_ok = len(raw) > 0
            return {
                "ok": True,
                "status": resp.status,
                "bytes": len(raw),
                "ms": round(ms, 1),
                "parse_ok": parse_ok,
            }
    except urllib.error.HTTPError as e:
        return {
            "ok": False,
            "status": int(e.code),
            "error": str(e.reason),
            "ms": round((time.perf_counter() - t0) * 1000, 1),
            "parse_ok": False,
        }
    except Exception as e:  # noqa: BLE001
        return {
            "ok": False,
            "status": 0,
            "error": str(e)[:240],
            "ms": round((time.perf_counter() - t0) * 1000, 1),
            "parse_ok": False,
        }


def stress_http_loop(
    label: str, url: str, duration_s: float, interval_s: float
) -> dict[str, Any]:
    ok = fail = parse_ok = parse_fail = 0
    latencies: list[float] = []
    t0 = time.monotonic()
    while time.monotonic() - t0 < duration_s:
        r = http_once(url)
        if r.get("ok"):
            ok += 1
            if r.get("parse_ok"):
                parse_ok += 1
            else:
                parse_fail += 1
            if "ms" in r:
                latencies.append(float(r["ms"]))
        else:
            fail += 1
        time.sleep(interval_s)
    out: dict[str, Any] = {
        "kind": "http",
        "label": label,
        "url": url,
        "http_ok": ok,
        "http_fail": fail,
        "parse_ok": parse_ok,
        "parse_fail": parse_fail,
    }
    if latencies:
        out["latency_ms_avg"] = round(sum(latencies) / len(latencies), 1)
        out["latency_ms_max"] = round(max(latencies), 1)
    return out


def stress_ws(
    label: str,
    url: str,
    duration_s: float,
    *,
    on_open: list[str] | None = None,
    query_key: str | None = None,
) -> dict[str, Any]:
    try:
        import asyncio

        import websockets  # type: ignore
    except ImportError:
        return {
            "kind": "ws",
            "label": label,
            "url": _redact(url),
            "skipped": True,
            "reason": "pip install websockets",
        }

    stats: dict[str, Any] = {
        "kind": "ws",
        "label": label,
        "url": _redact(url),
        "messages": 0,
        "text_frames": 0,
        "json_ok": 0,
        "json_fail": 0,
        "reconnects": 0,
        "errors": [],
        "auth_hints": 0,
        # 测站 /all 帧分类（避免把 heartbeat 当业务数据）
        "heartbeat": 0,
        "error_frames": 0,
        "meta_stations": 0,
        "data_int": 0,
        "data_mmi": 0,
        "data_other": 0,
        "max_data_gap_s": 0.0,
    }
    lock = threading.Lock()
    last_data_at: list[float | None] = [None]

    connect_url = url
    if query_key:
        sep = "&" if "?" in connect_url else "?"
        connect_url = f"{connect_url}{sep}key={query_key}"

    def classify(text: str) -> None:
        try:
            obj = json.loads(text)
        except json.JSONDecodeError:
            stats["json_fail"] += 1
            return
        stats["json_ok"] += 1
        if not isinstance(obj, dict):
            stats["data_other"] += 1
            return
        typ = str(obj.get("type") or "").lower()
        if typ in ("heartbeat", "pong", "ping"):
            stats["heartbeat"] += 1
            return
        if typ == "error" or "error" in obj:
            stats["error_frames"] += 1
            err = str(obj.get("message") or obj.get("error") or "")
            low = err.lower() + text.lower()
            if "expired_access_token" in low or "invalid_api_key" in low or "missing_api_key" in low:
                stats["auth_hints"] += 1
            return
        is_data = False
        if isinstance(obj.get("stations"), list):
            stats["meta_stations"] += 1
            is_data = True
        if isinstance(obj.get("int"), list):
            stats["data_int"] += 1
            is_data = True
        if isinstance(obj.get("mmi"), list):
            stats["data_mmi"] += 1
            is_data = True
        if not is_data:
            stats["data_other"] += 1
            is_data = typ not in ("",)
        if is_data:
            now = time.monotonic()
            if last_data_at[0] is not None:
                gap = now - last_data_at[0]
                if gap > float(stats["max_data_gap_s"]):
                    stats["max_data_gap_s"] = round(gap, 2)
            last_data_at[0] = now

    async def run() -> None:
        deadline = time.monotonic() + duration_s
        backoff = 1.0
        while time.monotonic() < deadline:
            try:
                async with websockets.connect(
                    connect_url,
                    open_timeout=20,
                    ping_interval=20,
                    ping_timeout=20,
                    max_size=8 * 1024 * 1024,
                    additional_headers={"User-Agent": UA},
                ) as ws:
                    backoff = 1.0
                    for text in on_open or []:
                        await ws.send(text)
                    while time.monotonic() < deadline:
                        remaining = deadline - time.monotonic()
                        if remaining <= 0:
                            break
                        try:
                            msg = await asyncio.wait_for(
                                ws.recv(), timeout=min(25.0, remaining)
                            )
                        except asyncio.TimeoutError:
                            try:
                                await ws.send("ping")
                            except Exception:  # noqa: BLE001
                                break
                            continue
                        with lock:
                            stats["messages"] += 1
                            if isinstance(msg, (bytes, bytearray)):
                                continue
                            stats["text_frames"] += 1
                            low = msg.lower()
                            if "expired_access_token" in low or "invalid_api_key" in low:
                                stats["auth_hints"] += 1
                            classify(msg)
            except Exception as e:  # noqa: BLE001
                with lock:
                    stats["reconnects"] += 1
                    stats["errors"].append(str(e)[:200])
                await asyncio.sleep(backoff)
                backoff = min(backoff * 2, 30.0)

    asyncio.run(run())
    stats["errors"] = stats["errors"][-12:]
    stats["max_data_gap_s"] = round(float(stats["max_data_gap_s"]), 2)
    return stats


def _redact(url: str) -> str:
    return re.sub(r"(key=)[^&]+", r"\1***", url)


def run_resource_check() -> dict[str, Any]:
    # 复用同目录脚本逻辑：子进程，避免 import 副作用
    import subprocess

    r = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "check_resources.py")],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    return {
        "kind": "local",
        "label": "check_resources",
        "ok": r.returncode == 0,
        "stdout": (r.stdout or "").strip()[:500],
        "stderr": (r.stderr or "").strip()[:500],
    }


def main() -> int:
    if not _truthy(os.environ.get("EEWVIEW_STRESS")):
        print("stress_live_sources: skip (set EEWVIEW_STRESS=1 to run)")
        return 0

    full = _truthy(os.environ.get("EEWVIEW_STRESS_FULL"))
    default_secs = "300" if full else "180"
    duration = float(os.environ.get("EEWVIEW_STRESS_SECS", default_secs))
    use_ws = _truthy(os.environ.get("EEWVIEW_STRESS_WS", "1"))

    cfg = parse_cfg(load_merged_toml_text())
    merge_user_settings_tokens(cfg)
    if full:
        # 全开：压测时不因配置关闭而跳过公开源
        for k in (
            "jian_enabled",
            "wolfx_enabled",
            "p2p_enabled",
            "pancakes_enabled",
            "typhoon_enabled",
            "map_wx_enabled",
            "weather_enabled",
            "jma_station",
            "kma_station",
            "snet",
        ):
            cfg[k] = True

    logs = ROOT / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
    report_path = logs / f"stress_live_{stamp}.json"

    report: dict[str, Any] = {
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "duration_s": duration,
        "full": full,
        "use_ws": use_ws,
        "jian_token_present": bool(cfg["has_jian_at"] or cfg["has_jian_rt"]),
        "results": [],
    }

    print(
        f"stress FULL={full} duration={duration}s ws={use_ws} "
        f"jian_token={report['jian_token_present']}"
    )

    # 1) 本地资源
    local = run_resource_check()
    report["results"].append(local)
    print(f"  local check_resources: {'OK' if local['ok'] else 'FAIL'}")

    # 2) HTTP 任务（并行）
    http_jobs: list[tuple[str, str, float]] = []
    u = cfg["urls"]
    if cfg["wolfx_enabled"]:
        http_jobs.append(("wolfx_eqlist", u["wolfx_eqlist"], 8.0))
        # Wolfx 另有 JMA 列表（公开 JSON）
        http_jobs.append(
            ("wolfx_jma_eqlist", "https://api.wolfx.jp/jma_eqlist.json", 12.0)
        )
    if cfg["p2p_enabled"]:
        base = u["p2p_http"].rstrip("?")
        http_jobs.append(("p2p_551", f"{base}?codes=551&limit=20", 10.0))
        http_jobs.append(("p2p_552", f"{base}?codes=552&limit=10", 15.0))
    if cfg["pancakes_enabled"]:
        base = u["pancakes_base"].rstrip("/")
        http_jobs.append(("pancakes_gq", f"{base}/api/v1/alert/quake/gq", 12.0))
        http_jobs.append(("pancakes_volcano", f"{base}/api/v1/alert/volcano/jma", 15.0))
    if cfg["typhoon_enabled"]:
        http_jobs.append(("typhoon", u["typhoon"], 12.0))
    if cfg.get("map_wx_enabled", True):
        # GET 限流较严：拉长间隔，避免整段 429
        http_jobs.append(("radar_cn", u["radar_cn"], 45.0))
        http_jobs.append(("radar_jp", u["radar_jp"], 50.0))
        http_jobs.append(("radar_tw", u["radar_tw"], 55.0))
        http_jobs.append(
            ("precip_24", f"{u['precip'].rstrip('?')}?period=24", 60.0)
        )

    # 3) WS 任务
    ws_jobs: list[tuple[str, str, list[str], str | None]] = []
    if use_ws:
        if cfg["wolfx_enabled"]:
            ws_jobs.append(
                (
                    "wolfx_all_eew",
                    u["wolfx_ws"],
                    ["ping", "query_jmaeew", "query_cenceqlist"],
                    None,
                )
            )
        if cfg["p2p_enabled"]:
            ws_jobs.append(("p2p_ws", u["p2p_ws"], [], None))
        if cfg["pancakes_enabled"]:
            ws_jobs.append(("pancakes_ws", u["pancakes_ws"], [], None))
        # /all、气象需 key；测站中转无需鉴权
        key = jian_query_key(cfg)
        if cfg["jian_enabled"]:
            ws_jobs.append(("jian_all", u["jian_ws"], ["ping"], key))
        if cfg.get("weather_enabled", True):
            ws_jobs.append(("weather", u["weather_ws"], ["ping"], key))
        if cfg["jma_station"]:
            ws_jobs.append(("kmoni", u["kmoni_ws"], [], None))
        if cfg["kma_station"]:
            ws_jobs.append(("kma_station", u["kma_ws"], [], None))
        if cfg["snet"]:
            ws_jobs.append(("snet", u["snet_ws"], [], None))

    results: list[dict[str, Any]] = []
    # HTTP 与 WS 同时跑满 duration（真正并行压测）
    with ThreadPoolExecutor(max_workers=max(4, len(http_jobs) + len(ws_jobs))) as pool:
        futs = []
        for label, url, interval in http_jobs:
            print(f"  HTTP {label} -> {url}")
            futs.append(pool.submit(stress_http_loop, label, url, duration, interval))
        for label, url, on_open, key in ws_jobs:
            print(f"  WS   {label} -> {_redact(url)}")
            futs.append(
                pool.submit(stress_ws, label, url, duration, on_open=on_open, query_key=key)
            )
        for fut in as_completed(futs):
            try:
                results.append(fut.result())
            except Exception as e:  # noqa: BLE001
                results.append({"kind": "error", "error": str(e)[:300]})

    report["results"].extend(results)
    report["finished_utc"] = datetime.now(timezone.utc).isoformat()

    # 汇总
    summary = {"http_fail_labels": [], "ws_auth_labels": [], "ws_skipped": [], "hard_fail": False}
    for r in report["results"]:
        if r.get("label") == "check_resources" and not r.get("ok"):
            summary["hard_fail"] = True
        if r.get("kind") == "http":
            if r.get("http_ok", 0) == 0 and r.get("http_fail", 0) > 0:
                summary["http_fail_labels"].append(r.get("label"))
                summary["hard_fail"] = True
            print(
                f"  result HTTP {r.get('label')}: ok={r.get('http_ok')} fail={r.get('http_fail')} "
                f"parse_ok={r.get('parse_ok')}"
            )
        elif r.get("kind") == "ws":
            if r.get("skipped"):
                summary["ws_skipped"].append(r.get("label"))
            if r.get("auth_hints", 0) > 0:
                summary["ws_auth_labels"].append(r.get("label"))
            print(
                f"  result WS {r.get('label')}: msgs={r.get('messages')} "
                f"hb={r.get('heartbeat')} int={r.get('data_int')} mmi={r.get('data_mmi')} "
                f"meta={r.get('meta_stations')} err={r.get('error_frames')} "
                f"gap_max={r.get('max_data_gap_s')}s reconnects={r.get('reconnects')} "
                f"auth={r.get('auth_hints')} skipped={r.get('skipped', False)}"
            )
    report["summary"] = summary
    report_path.write_bytes(json.dumps(report, ensure_ascii=False, indent=2).encode("utf-8"))
    print(f"stress: report -> {report_path.relative_to(ROOT)}")
    return 1 if summary["hard_fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
