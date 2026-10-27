<p align="center">
  <img src="icon/brand/app.png" alt="StarQuake" width="128" />
</p>

<h1 align="center">StarQuake</h1>

<p align="center">
  纯 Rust 桌面端 · 多源预警 / 速报视监
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License" /></a>
  <img src="https://img.shields.io/badge/Rust-1.70%2B-orange.svg" alt="Rust" />
  <img src="https://img.shields.io/badge/Platform-Windows-0078D6.svg" alt="Windows" />
  <img src="https://img.shields.io/badge/Release-Beta_2.3.4-informational.svg" alt="Beta 2.3.4" />
</p>

<p align="center">
  <img src="https://socialify.git.ci/Jian11323/StarQuake/image?description=1&amp;font=Inter&amp;language=1&amp;name=1&amp;owner=1&amp;pattern=Solid&amp;theme=Auto" alt="StarQuake" width="640" />
</p>

---

## 简介

**StarQuake** 是一款面向桌面的地震预警与速报视监客户端。  
聚合多源实时情报，在地图上展示震中、走时波圈与测站烈度，并提供本机倒计时与音效播报。

---

## 开发者的话

> 大家好，我是**纪安**。这是我使用 AI 开发的第二个软件。如果存在不好的地方请多多包涵。
>
> 编程也算是我的一个小爱好，也是我在休息时间逃避题海、放松的一个方式。虽然一开始对这个软件没抱很大的期望，但是在 AI 的帮助下真的搓出了一个像模像样的软件。虽说这个软件依然存在很多问题。
>
> 这个软件和[地震情报实况栏](https://github.com/Jian11323/Rolling-Subtitle)一样，也是「Cursor」的产物。
>
> 所以，**StarQuake 的所有文件内容，全部由 AI 编写完成**，我并没有为软件编写任何一行代码，仅进行了架构设计与修改部分文字描述，因此我有必要添加以下声明：

> [!WARNING]
> 本软件和文档由 AI 生成，内容仅供参考，请仔细甄别。
>
> 软件目前仍处于开发测试阶段，无法 100% 保证稳定性与可用性。

> 这个软件历经几十次的推倒重来以及各种修改，才终于打磨出一个相对稳定的版本。真的很感谢 AI，没有它这个项目根本做不出来。这个软件算是我和 AI 一起折腾出来的成果。它远远谈不上完美，但至少整体框架是稳固的，代码逻辑是清晰的。
>
> 希望这个软件能在地震防灾上给大家带来一点点帮助。也欢迎各位大佬来测试、提意见、帮忙改进，多多指点。如果你觉得这个为爱发电的小工具还算实用，或是喜欢这个项目，**麻烦给仓库点一颗** ⭐ **Star** ⭐，对我来说就是很大的鼓励！

> [!NOTE]
> 虽然这个软件在开发的过程中大量使用 AI 进行辅助，但我保证，AI 生成的每行代码都经过严格审查，请各位放心使用。

---

## 主要功能

- 接收中国地震局、台湾中央气象署、日本气象厅、韩国气象厅等地震预警
- 接收中国地震台网、日本气象厅等地震速报 / 列表
- 接收日本气象厅海啸情报
- 接收中央台气象预警（侧栏「气象」：图标 / 标题 / 省市区 / 正文 + 地图色点）
- 展示 NIED 強震モニタ、S-Net、KMA 等测站烈度（地图色点与右上角台网卡片）
- 地图叠加震中、P/S 波圈（近距 JMA2001，远距 ak135）
- 暗色矢量底图 / 亮色海洋地形瓦片，可随主题切换
- 台风路径、风圈与多源预报（可开关）
- 地图雷达叠加（中国 / 日本 / 台湾可独立开关，设置 → 地图显示；GET）与中国降水落区 24/48/72 小时；雷达/降水透明度可调
- 地图晨昏线（设置 → 地图显示，默认关）；设置本机经纬度后在右下角显示预估日出与日落
- 配置本机经纬度（地图选点或按公网 IP 定位）后估算本地 S 波倒计时，并支持音效播报
- 预警/速报 TTS：本机离线语音包（Windows SAPI）或微软 Edge 在线 Neural TTS
- 各数据源健康状态：**正常** / **波动** / **异常**
- 界面语言切换：简体中文 / 繁体中文 / 日本語 / English（设置 → 通用）
- 右侧情报侧栏可开关（设置、向右/向下拖关闭或右缘拉开）；拖动标题可分离为独立窗口（嵌回：按钮 / 双击标题 / 拖近主窗右侧 / 关浮窗）；竖屏时内嵌为底栏；列表按 eewcn 方式随宽度换行重排行高（不缩放字号）；地图预警条/海啸条/右上实时震度与台风在小视口缩小、大屏保持设计尺寸（图例与右下时钟固定）；关闭或分离后地图右键可打开设置、侧栏、跳转所在地、图例、设置所在地与关于
- 主窗内嵌设置（数据源、地图与显示、音频、性能、高级、通知中心、关于）
- 官方通知中心：每 15 秒拉取 [Jian Project 通知](https://api.sismotide.top/api/v1/notifications/updates)，未读弹窗 + 设置页列表（与地震情报实况栏共用）
- 自定义告警音：NHK 一级新闻铃、JMA 警报音、津波紧急警报放送；可开关
- 桌面系统通知（预警 / 速报 / 海啸，可分别开关）

---

## 数据来源

- 地震预警（CEA/CWA/JMA/KMA 等）、地震速报、海啸情报、实时测站、台风路径、气象预警：[Jian Project API](https://api.sismotide.top/api/)
- 地震预警（CEA/SC/FJ/CWA/JMA/CENC）、地震列表（CENC/JMA）：[Wolfx Open API](https://wolfx.jp/api/)（请注意参考接口文档）
- 地震信息（JMA）：[P2PQuake](https://www.p2pquake.net/)
- 地震信息（FSSN）：[FSSN WebSocket API](https://fssn-api.fanstudio.tech/#fssn)
- GlobalQuake 全球预警、JMA-Atom 火山情报：[PancakesAPI](https://wiki.aloys23.link/wiki/PancakesAPI)
- 日本测站烈度：[NIED 強震モニタ](https://www.lmoni.bosai.go.jp/)
- 亮色海洋地形瓦片：[sismotide tilemap](https://tilemap.sismotide.top/)
- 地图雷达 / 降水落区：[sismotide GET](https://api.sismotide.top/get/)（`radar.json` / `radar-japan.json` / `radar-taiwan.json` / `precip-forecast.json`）
- IP 归属定位：[sismotide GET IP](https://api.sismotide.top/get/ip.json)
- 中国大陆地图：[阿里云 DataV.GeoAtlas](https://datav.aliyun.com/portal/school/atlas/area_selector)
- 中国台湾地图：[GeoJSON.cn](https://geojson.cn/)
- 日本地图：[日本气象厅](https://www.data.jma.go.jp/developer/gis.html)（注意钓鱼岛地区处理）
- 世界地图：[GeoJSON Maps of the globe](https://geojson-maps.kyd.au/)（注意甄别争议地区）
- 全球活动断层：[GEM Global Active Faults](https://github.com/GEMScienceTools/gem-global-active-faults)
- 中国活动断层：[gmt-china/china-geospatial-data](https://github.com/gmt-china/china-geospatial-data)（CAFD）
- 近距走时表：[气象厅 JMA2001](https://www.data.jma.go.jp/eqev/data/bulletin/catalog/appendix/trtime/trt_j.html)
- 远距走时表：IASPEI ak135
- SREV 音效（JMA）：[scratch-realtime-earthquake-viewer-page](https://github.com/kotoho7/scratch-realtime-earthquake-viewer-page)
- Open EEW CN 音效（非 JMA 预警 / 速报）：[OpenEEWCN/eewcn](https://github.com/OpenEEWCN/eewcn)
- 中文倒计时播报素材：[地牛 Wake Up!](https://eew.earthquake.tw/)
- 界面字体：[霞鹜文楷](https://github.com/lxgw/LxgwWenKai)（SIL OFL 1.1）

---

## 参考软件

- [Open EEW CN](https://github.com/OpenEEWCN/eewcn)
- [GlobalQuake 0.11](https://github.com/xspanger3770/GlobalQuake)
- [Kanameishi](https://github.com/Lipomoea/kanameishi)
- [RhythmQuake](https://github.com/yuelinyayun-star/flutterRhythmQuake)
- [SCEEW](https://github.com/TenkyuChimata/SCEEW)
- [Scratch-Realtime-Earthquake-Viewer-Page](https://github.com/kotoho7/scratch-realtime-earthquake-viewer-page)

---

## 版权声明

- [RhythmQuake](https://github.com/yuelinyayun-star/flutterRhythmQuake)
- [Kanameishi](https://github.com/Lipomoea/kanameishi)
- [OPEN EEWCN](https://github.com/OpenEEWCN/eewcn)
- [GlobalQuake 0.11](https://github.com/OpenEEWCN/eewcn)

第三方素材（音效、走时表、底图矢量、断层、字体、地图标记等）不随代码许可证覆盖，请遵循各自上游许可；SREV 音效为 CC BY-SA 2.0，须署名。列表/图例震度色与 `icon/intensity` 图标一致；中国烈度色对齐 EEWCN；测站点仍用 NIED 計測色带。

---

## 特别鸣谢

- [平平无奇的4](https://space.bilibili.com/3707038962027155)
- [悦琳雅韵☆Rhythm(Stille)](https://github.com/yuelinyayun-star)
- [lxfly2000](https://github.com/lxfly2000)
- 以及各位给予帮助的 EEW 爱好者

--- 

## 注意事项

1. **非官方渠道**：本软件仅供信息展示与学习研究，**不构成**各国气象 / 地震主管部门的官方预警渠道。紧急情况下请以主管部门发布为准，并遵从当地防灾指引。
2. **数据免责**：开发者不保证上游数据的实时性、完整性与准确性；网络波动、上游变更或连接超限均可能导致延迟、缺报或显示异常。
3. **令牌与隐私**：Jian 访问令牌请仅保存在本机设置或环境变量中，**勿提交进公开仓库**。
4. **素材授权**：部分音效为非自由许可，默认不随仓库分发；公开再分发前请自行确认授权或更换素材。
5. **连接额度**：同一 IP / 令牌并发有限。若同时开启多个客户端或调试页，可能触发上游连接限制；请关闭多余连接后再试。

## 版权声明

本仓库代码以 [Apache License 2.0](LICENSE) 发布。
