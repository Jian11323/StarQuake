# 要石（kanameishi）暗色矢量底图

源自 [Lipomoea/kanameishi](https://github.com/Lipomoea/kanameishi)（AGPL-3.0）的 `public/json/*.topo.json`。  
数据快照与许可说明见同目录 [`NOTICE.md`](NOTICE.md)、[`KA-LICENSE.txt`](KA-LICENSE.txt)（与 RhythmQuake `assets/maps/ka/` 对齐）。

| 文件 | 说明 |
|------|------|
| `medium.global.modified.topo.json` | 世界底图 |
| `cn.province.topo.json` | 中国省级 |
| `jp.pref.topo.json` | 日本都道府县 |
| `cn.eew.topo.json` | 中国 EEW 烈度区划（市县级；与 RhythmQuake 同源） |
| `jp.eew.topo.json` | 日本 EEW 预报区（JMA class10 级；warnArea 填色） |

运行时直接解析 TopoJSON（`starquake-map`），无需再展开 GeoJSON。  
暗色绘制令牌：海洋 `#202124`、陆地 `#393939`、边界 `#BBBBBB`。
