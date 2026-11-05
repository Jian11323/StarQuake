# 行政区 / 底图数据

## 暗色矢量（默认）

`basemap = "auto"` 且主题 `dark` 时加载矢量世界轮廓 + 中日**省级**区划 + 国外一级行政区（州/省）。

平面与球体按当前投影**分别加载**，切换投影时重载，避免两套几何同时常驻。

### 世界轮廓

| 路径 | 说明 |
|------|------|
| `kanameishi/medium.global.modified.topo.json` | 要石精细世界（不含中日国界） |

另叠内置钓鱼岛点状。世界层在加载时剥离藏南/麦克马洪线走廊（并丢弃断头残段）；中日国级外轮廓由省级区划溶解得到（含藏南），始终加载；zoom ≥ 7.5 时再叠省级/州级描边。

### 区划层

| 路径 | 内容 |
|------|------|
| `globalquake/cn-prefectures.geojson` | 中国省级（含藏南；优先） |
| `kanameishi/jp.pref.topo.json` | 日本都道府县（JMA；优先，含北方四岛） |
| `globalquake/jp-prefectures.geojson` | 日本都道府县回退（无北方四岛） |
| 要石 `cn.province` | 中国省级回退 |
| `admin1/states_provinces.geojson` | 国外一级行政区（美加澳等；Natural Earth 50m 精简） |

钓鱼岛由中国台湾省界与内置点状表达；日本区划中落入该范围的小岛环在加载时剥除。
