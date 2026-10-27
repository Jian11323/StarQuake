# -*- coding: utf-8 -*-
"""下载并简化活动断层 GeoJSON 到 geodata/faults/。

产出：
- `*_active_faults.geojson`：全部分段（近距 fine）
- `*_active_faults_lod0.geojson`：长断层 / 高滑移板缘（远距）
- `*_active_faults_lod1.geojson`：中等断层（中距）

全球源优先用本机 GlobalQuake 的 GEM harmonized（与 GQ 一致），
否则下载 GEM 仓库 geojson；中国用 gmt-china CN-faults（CAFD），
并合并 GEM 中国邻区段以补齐跨境断层；全球层另并入国土地理院 HERP 日本主要活断层带。

也可对已有 fine 只生成 LOD：`python tools/prep_faults.py --lod-only`
"""
from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import ssl
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FAULTS = ROOT / "geodata" / "faults"

GEM_URL = (
    "https://raw.githubusercontent.com/GEMScienceTools/gem-global-active-faults/"
    "master/geojson/gem_active_faults_harmonized.geojson"
)
GEM_URL_FALLBACK = (
    "https://raw.githubusercontent.com/GEMScienceTools/gem-global-active-faults/"
    "master/geojson/gem_active_faults.geojson"
)

# 中国 CAFD：GitHub raw 常 SSL 失败，优先 jsDelivr
CN_URLS = [
    "https://cdn.jsdelivr.net/gh/gmt-china/china-geospatial-data@main/CN-faults.gmt",
    "https://raw.githubusercontent.com/gmt-china/china-geospatial-data/main/CN-faults.gmt",
]

GQ_LOCAL_CANDIDATES = [
    Path(r"D:\Program Files (x86)\GlobalQuake\GlobalQuake\faults\gem_active_faults_harmonized.geojson"),
    Path(r"D:\Program Files (x86)\GlobalQuake\GlobalQuake1\faults\gem_active_faults_harmonized.geojson"),
]

LOD0_MIN_DIAG = 1.0
LOD1_MIN_DIAG = 0.28
LOD0_MIN_SLIP = 5.0

# 并入中国层的 GEM 邻区（含台日琉球西缘），补 CN-faults 跨境缺口
CN_MERGE_BBOX = (70.0, 15.0, 135.0, 54.0)


def _proxy() -> str | None:
    return (
        os.environ.get("HTTPS_PROXY")
        or os.environ.get("HTTP_PROXY")
        or os.environ.get("https_proxy")
        or os.environ.get("http_proxy")
        or "http://127.0.0.1:7890"
    )


def download(url: str, dest: Path, *, retries: int = 3) -> None:
    print(f"download {url}")
    dest.parent.mkdir(parents=True, exist_ok=True)
    proxy = _proxy()
    last_err: Exception | None = None
    for attempt in range(1, retries + 1):
        for use_proxy in (True, False):
            try:
                handlers: list = []
                if use_proxy and proxy:
                    handlers.append(
                        urllib.request.ProxyHandler({"http": proxy, "https": proxy})
                    )
                else:
                    handlers.append(urllib.request.ProxyHandler({}))
                ctx = ssl.create_default_context()
                opener = urllib.request.build_opener(
                    *handlers, urllib.request.HTTPSHandler(context=ctx)
                )
                with opener.open(url, timeout=180) as r:
                    dest.write_bytes(r.read())
                print(f"  -> {dest} ({dest.stat().st_size} bytes) proxy={use_proxy}")
                return
            except Exception as e:  # noqa: BLE001
                last_err = e
                print(f"  attempt {attempt} proxy={use_proxy} failed: {type(e).__name__}: {e}")
        time.sleep(1.2 * attempt)
    raise RuntimeError(f"download failed: {url}") from last_err


def download_first(urls: list[str], dest: Path) -> None:
    errs: list[str] = []
    for url in urls:
        try:
            download(url, dest)
            return
        except Exception as e:  # noqa: BLE001
            errs.append(f"{url}: {e}")
    raise RuntimeError("all mirrors failed:\n" + "\n".join(errs))


def parse_ogr_gmt(path: Path) -> list[dict]:
    text = path.read_text(encoding="utf-8", errors="replace")
    features: list[dict] = []
    chunks = re.split(r"(?m)^>\s*$", text)
    for chunk in chunks:
        lines = [ln.strip() for ln in chunk.splitlines() if ln.strip()]
        if not lines:
            continue
        props: dict = {}
        coords: list[list[float]] = []
        for ln in lines:
            if ln.startswith("#"):
                if ln.startswith("# @N") or ln.startswith("#@N"):
                    props["_names"] = ln.split("N", 1)[1].strip().split("|")
                elif ln.startswith("# @D") or ln.startswith("#@D"):
                    vals = ln.split("D", 1)[1].strip().split("|")
                    names = props.pop("_names", None)
                    if isinstance(names, list):
                        for i, name in enumerate(names):
                            if i < len(vals) and name:
                                props[name] = vals[i]
                continue
            parts = ln.split()
            if len(parts) >= 2:
                try:
                    lon = float(parts[0])
                    lat = float(parts[1])
                except ValueError:
                    continue
                if -180.0 <= lon <= 180.0 and -90.0 <= lat <= 90.0:
                    coords.append([lon, lat])
        if len(coords) >= 2:
            features.append(
                {
                    "type": "Feature",
                    "properties": {k: v for k, v in props.items() if not str(k).startswith("_")},
                    "geometry": {"type": "LineString", "coordinates": coords},
                }
            )
    return features


def simplify_line(coords: list[list[float]], tol_deg: float) -> list[list[float]]:
    if len(coords) <= 2 or tol_deg <= 0:
        return coords

    def perp_dist(p, a, b):
        ax, ay = a
        bx, by = b
        px, py = p
        dx, dy = bx - ax, by - ay
        if dx == 0 and dy == 0:
            return math.hypot(px - ax, py - ay)
        t = ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)
        t = max(0.0, min(1.0, t))
        return math.hypot(px - (ax + t * dx), py - (ay + t * dy))

    def dp(pts):
        if len(pts) <= 2:
            return pts
        a, b = pts[0], pts[-1]
        idx, mx = 0, 0.0
        for i in range(1, len(pts) - 1):
            d = perp_dist(pts[i], a, b)
            if d > mx:
                idx, mx = i, d
        if mx > tol_deg:
            left = dp(pts[: idx + 1])
            right = dp(pts[idx:])
            return left[:-1] + right
        return [a, b]

    return dp(coords)


def extract_lines(geom: dict) -> list[list[list[float]]]:
    t = geom.get("type")
    c = geom.get("coordinates")
    if not t or c is None:
        return []
    if t == "LineString":
        return [c] if len(c) >= 2 else []
    if t == "MultiLineString":
        return [ln for ln in c if len(ln) >= 2]
    return []


def line_diag(coords: list[list[float]]) -> float:
    if len(coords) < 2:
        return 0.0
    xs = [p[0] for p in coords]
    ys = [p[1] for p in coords]
    return math.hypot(max(xs) - min(xs), max(ys) - min(ys))


def line_length(coords: list[list[float]]) -> float:
    s = 0.0
    for a, b in zip(coords, coords[1:]):
        s += math.hypot(b[0] - a[0], b[1] - a[1])
    return s


def line_bbox(coords: list[list[float]]) -> tuple[float, float, float, float]:
    xs = [p[0] for p in coords]
    ys = [p[1] for p in coords]
    return min(xs), min(ys), max(xs), max(ys)


def bbox_overlap(a: tuple[float, float, float, float], b: tuple[float, float, float, float]) -> bool:
    return not (a[2] < b[0] or a[0] > b[2] or a[3] < b[1] or a[1] > b[3])


def parse_slip_rate(props: dict) -> float:
    v = props.get("net_slip_rate")
    if v is None:
        v = props.get("slip_rate")
    if v is None:
        v = props.get("slipRate")
    if v is None:
        return 0.0
    if isinstance(v, (int, float)):
        return float(max(0.0, min(100.0, v)))
    s = str(v).strip()
    if not s or s.lower() in ("none", "null", "-", "nan"):
        return 0.0
    if s.startswith("("):
        s = s[1:]
    head = re.split(r"[, \-\u2013]", s)[0].strip()
    try:
        return float(max(0.0, min(100.0, float(head))))
    except ValueError:
        return 0.0


KEEP_PROP_KEYS = (
    "net_slip_rate",
    "slip_rate",
    "slipRate",
    "slip_type",
    "name",
    "fs_name",
    "catalog_name",
)


def slim_props(raw: dict, source: str) -> dict:
    props = {"src": source}
    for key in KEEP_PROP_KEYS:
        if key in raw and raw[key] is not None:
            props[key] = raw[key]
    return props


def slim_collection(features: list[dict], tol: float, source: str) -> dict:
    out = []
    n_in = n_pts_in = n_pts_out = 0
    for f in features:
        geom = f.get("geometry") or {}
        raw_props = f.get("properties") or {}
        for line in extract_lines(geom):
            n_in += 1
            n_pts_in += len(line)
            pts = [[float(p[0]), float(p[1])] for p in line if len(p) >= 2]
            # 短折线/板缘 2 点段不简化，避免日本海沟几何被掏空
            use_tol = 0.0 if len(pts) <= 4 else tol
            slim = simplify_line(pts, use_tol)
            if len(slim) < 2:
                continue
            n_pts_out += len(slim)
            out.append(
                {
                    "type": "Feature",
                    "properties": slim_props(raw_props, source),
                    "geometry": {"type": "LineString", "coordinates": slim},
                }
            )
    print(f"{source}: lines={n_in} pts {n_pts_in}->{n_pts_out} out={len(out)}")
    return {"type": "FeatureCollection", "features": out}


def write_fc(path: Path, fc: dict) -> None:
    path.write_text(
        json.dumps(fc, ensure_ascii=False, separators=(",", ":")), encoding="utf-8"
    )
    print(f"wrote {path} ({path.stat().st_size} bytes)")


def emit_lod_from_fine(fine_path: Path, stem: str, source: str) -> None:
    fc = json.loads(fine_path.read_text(encoding="utf-8"))
    feats = fc.get("features") or []
    lod0, lod1 = [], []
    for f in feats:
        geom = f.get("geometry") or {}
        lines = extract_lines(geom)
        if not lines:
            continue
        best = max(lines, key=lambda ln: max(line_diag(ln), line_length(ln)))
        diag = line_diag(best)
        length = line_length(best)
        props = slim_props(dict(f.get("properties") or {}), source)
        # 保留 fine 里已有 src
        if isinstance(f.get("properties"), dict) and f["properties"].get("src"):
            props["src"] = f["properties"]["src"]
        slip = parse_slip_rate(props)
        size = max(diag, length)
        src = str(props.get("src") or source)
        # HERP 日本主要活断层多为短段且无滑移字段：全部至少进 lod1（中距可见）
        if src == "jpherp":
            if size >= 0.55:
                size = max(size, LOD0_MIN_DIAG)
            else:
                size = max(size, LOD1_MIN_DIAG)
        if size >= LOD0_MIN_DIAG or slip >= LOD0_MIN_SLIP:
            slim = simplify_line(
                [[float(p[0]), float(p[1])] for p in best if len(p) >= 2],
                0.0 if len(best) <= 4 else 0.04,
            )
            if len(slim) >= 2:
                lod0.append(
                    {
                        "type": "Feature",
                        "properties": props,
                        "geometry": {"type": "LineString", "coordinates": slim},
                    }
                )
        elif size >= LOD1_MIN_DIAG or slip >= 1.0:
            slim = simplify_line(
                [[float(p[0]), float(p[1])] for p in best if len(p) >= 2],
                0.0 if len(best) <= 4 else 0.015,
            )
            if len(slim) >= 2:
                lod1.append(
                    {
                        "type": "Feature",
                        "properties": props,
                        "geometry": {"type": "LineString", "coordinates": slim},
                    }
                )
    write_fc(
        FAULTS / f"{stem}_lod0.geojson",
        {"type": "FeatureCollection", "features": lod0},
    )
    write_fc(
        FAULTS / f"{stem}_lod1.geojson",
        {"type": "FeatureCollection", "features": lod1},
    )
    print(f"{stem} lod: lod0={len(lod0)} lod1={len(lod1)} fine={len(feats)}")


def emit_all_lod() -> None:
    emit_lod_from_fine(
        FAULTS / "global_active_faults.geojson", "global_active_faults", "gem"
    )
    emit_lod_from_fine(
        FAULTS / "china_active_faults.geojson", "china_active_faults", "cn"
    )



def fetch_japan_gsi_features() -> list[dict]:
    """国土地理院矢量切片：地震调查研究推进本部「全国的主要活断层带」。"""
    # maxNativeZoom=2，实际数据落在少数瓦片（实测 2/3/1）
    # 实测全国数据在 2/3/1；其余多为 404，仅作回退扫描
    urls = ["https://maps.gsi.go.jp/xyz/active_fault/2/3/1.geojson"] + [
        f"https://maps.gsi.go.jp/xyz/active_fault/2/{x}/{y}.geojson"
        for x in range(0, 4)
        for y in range(0, 4)
        if not (x == 3 and y == 1)
    ]
    out: list[dict] = []
    for url in urls:
        try:
            dest = FAULTS / "_tmp_jp_tile.geojson"
            download(url, dest, retries=2)
            fc = json.loads(dest.read_text(encoding="utf-8"))
            dest.unlink(missing_ok=True)
            for f in fc.get("features") or []:
                geom = f.get("geometry") or {}
                for line in extract_lines(geom):
                    pts = [[float(p[0]), float(p[1])] for p in line if len(p) >= 2]
                    if len(pts) < 2:
                        continue
                    out.append(
                        {
                            "type": "Feature",
                            "properties": {"src": "jpherp", "name": "HERP major active fault"},
                            "geometry": {"type": "LineString", "coordinates": pts},
                        }
                    )
        except Exception as e:
            print(f"japan tile skip {url}: {e}")
    print(f"japan HERP/GSI features: {len(out)}")
    return out


def resolve_gem_source(raw_gem: Path) -> None:
    for cand in GQ_LOCAL_CANDIDATES:
        if cand.is_file():
            print(f"use local GQ faults: {cand}")
            shutil.copy2(cand, raw_gem)
            return
    try:
        download(GEM_URL, raw_gem)
    except Exception as e:
        print(f"harmonized download failed: {e}; try fallback")
        download(GEM_URL_FALLBACK, raw_gem)


def gem_features_in_bbox(gem_feats: list[dict], bbox: tuple[float, float, float, float]) -> list[dict]:
    out = []
    for f in gem_feats:
        geom = f.get("geometry") or {}
        for line in extract_lines(geom):
            pts = [[float(p[0]), float(p[1])] for p in line if len(p) >= 2]
            if len(pts) < 2:
                continue
            if bbox_overlap(line_bbox(pts), bbox):
                out.append(
                    {
                        "type": "Feature",
                        "properties": dict(f.get("properties") or {}),
                        "geometry": {"type": "LineString", "coordinates": pts},
                    }
                )
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description="Prepare active fault GeoJSON + LOD")
    ap.add_argument(
        "--lod-only",
        action="store_true",
        help="只从已有 fine GeoJSON 生成 lod0/lod1，不下载上游",
    )
    args = ap.parse_args()
    FAULTS.mkdir(parents=True, exist_ok=True)

    if args.lod_only:
        emit_all_lod()
        print("done (lod-only)")
        return

    raw_gem = FAULTS / "_raw_gem_active_faults.geojson"
    raw_cn = FAULTS / "_raw_CN-faults.gmt"

    resolve_gem_source(raw_gem)
    download_first(CN_URLS, raw_cn)

    cn_feats = parse_ogr_gmt(raw_cn)
    if not cn_feats:
        raise SystemExit("CN-faults parse produced 0 features")

    gem = json.loads(raw_gem.read_text(encoding="utf-8"))
    gem_feats = gem.get("features") or []

    # 中国：CAFD 低简化 + GEM 邻区（含滑移），补齐跨境与东北亚
    cn_fc = slim_collection(cn_feats, 0.002, "cn")
    gem_cn = slim_collection(gem_features_in_bbox(gem_feats, CN_MERGE_BBOX), 0.006, "gem")
    merged_cn = {
        "type": "FeatureCollection",
        "features": (cn_fc.get("features") or []) + (gem_cn.get("features") or []),
    }
    print(
        f"china merge: cn={len(cn_fc.get('features') or [])} "
        f"+ gem_bbox={len(gem_cn.get('features') or [])} "
        f"-> {len(merged_cn['features'])}"
    )
    write_fc(FAULTS / "china_active_faults.geojson", merged_cn)

    # 全球：GEM（含滑移）+ 日本 HERP/GSI 主要活断层带
    global_fc = slim_collection(gem_feats, 0.01, "gem")
    jp_feats = fetch_japan_gsi_features()
    if jp_feats:
        jp_fc = slim_collection(jp_feats, 0.004, "jpherp")
        global_fc = {
            "type": "FeatureCollection",
            "features": (global_fc.get("features") or []) + (jp_fc.get("features") or []),
        }
        print(
            f"global merge: gem={len(gem_feats)} + jpherp={len(jp_fc.get('features') or [])} "
            f"-> {len(global_fc['features'])}"
        )
    write_fc(FAULTS / "global_active_faults.geojson", global_fc)

    raw_gem.unlink(missing_ok=True)
    raw_cn.unlink(missing_ok=True)
    emit_all_lod()
    print("done (raw intermediates removed)")


if __name__ == "__main__":
    main()
