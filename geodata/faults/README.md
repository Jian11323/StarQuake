# 活动断层数据

运行时加载：

| 文件 | 内容 |
|------|------|
| `global_active_faults.geojson` | 全球活动断层（由 GEM GAF-DB 简化，近距 fine） |
| `global_active_faults_lod0.geojson` | 全球长断层（远距） |
| `global_active_faults_lod1.geojson` | 全球中等断层（中距） |
| `china_active_faults.geojson` | 中国及邻区活断层（由 CAFD2023 / CN-faults 简化，近距 fine） |
| `china_active_faults_lod0.geojson` | 中国长断层（远距） |
| `china_active_faults_lod1.geojson` | 中国中等断层（中距） |

平面图与球体图共用；设置项 `ui.show_faults` 默认关闭，开启后才加载。缺 lod 文件时运行时从 fine 按对角线长度与滑移速率现场分档。

属性：全球/并入中国层的 GEM 段保留 `net_slip_rate`（mm/yr），线宽按 GlobalQuake 公式随滑移加粗；高滑移短段（如日本海沟）进入 lod0。

## 来源与许可

| 层 | 上游 | 许可 / 引用 |
|----|------|-------------|
| 全球 | [GEM Global Active Faults](https://github.com/GEMScienceTools/gem-global-active-faults)（优先本机 GQ `gem_active_faults_harmonized`） | **CC BY-SA 4.0**；Styron & Pagani (2020), *Earthquake Spectra* |
| 中国 | [gmt-china/china-geospatial-data](https://github.com/gmt-china/china-geospatial-data) `CN-faults` ← CAFD2023，并合并 GEM 中国邻区段 | 徐锡伟团队 CAFD；Wu et al. (2024), *ESSD*；[DOI](https://doi.org/10.5194/essd-16-3391-2024) |
| 日本补充 | 国土地理院矢量切片 `active_fault`（地震调查研究推进本部「全国的主要活断层带」） | 出典表記；[GSI](https://maps.gsi.go.jp/) / [HERP](https://www.jishin.go.jp/) |

## 重新生成

```bash
python tools/prep_faults.py
```

仅从已有 fine 生成 lod0/lod1（不下载上游）：

```bash
python tools/prep_faults.py --lod-only
```

脚本会下载上游原始数据（中国层优先 jsDelivr 镜像）、简化几何并写出上述 GeoJSON（不把原始大文件提交进仓库）。国内直连 GitHub 失败时请开本机代理 `http://127.0.0.1:7890`。
