//! 机构展示标签与过滤族。
//!
//! 预警（EEW）与速报（信息事件）分族：`cea` 展示为 `[CN]`，`cenc` 展示为 `[CENC]`。

/// Jian `/all` 预警子源（设置页挂在数据源下方；GQ 由 Pancakes 开关负责）。
pub const EEW_FILTER_FAMILIES: &[AgencyFamily] = &[
    AgencyFamily::Cea,
    AgencyFamily::CeaPr,
    AgencyFamily::Cwa,
    AgencyFamily::Jma,
    AgencyFamily::Kma,
    AgencyFamily::Sa,
    AgencyFamily::EarlyEst,
];

/// 信息事件震级过滤（Jian `/all` 内各地震速报源，对齐 RhythmQuake）。
pub const RECORD_FILTER_FAMILIES: &[AgencyFamily] = &[
    AgencyFamily::Cenc,
    AgencyFamily::Usgs,
    AgencyFamily::Cwa,
    AgencyFamily::Jma,
    AgencyFamily::Hko,
    AgencyFamily::Emsc,
    AgencyFamily::Kma,
    AgencyFamily::Ningxia,
    AgencyFamily::Yunnan,
    AgencyFamily::Shanxi,
    AgencyFamily::Beijing,
    AgencyFamily::Gfz,
    AgencyFamily::Bcsf,
    AgencyFamily::Usp,
    AgencyFamily::Ingv,
    AgencyFamily::Tmd,
    AgencyFamily::Mmd,
    AgencyFamily::Bmkg,
    AgencyFamily::Geonet,
    AgencyFamily::Afad,
    AgencyFamily::Ipma,
    AgencyFamily::Noa,
    AgencyFamily::Sed,
    AgencyFamily::Scsn,
    AgencyFamily::Ipgp,
    AgencyFamily::Infp,
    AgencyFamily::Isc,
    AgencyFamily::Knmi,
    AgencyFamily::Ncedc,
    AgencyFamily::Lmu,
    AgencyFamily::Koeri,
    AgencyFamily::Gsras,
    AgencyFamily::Csn,
    AgencyFamily::Phivolcs,
    AgencyFamily::Ssn,
    AgencyFamily::Ga,
    AgencyFamily::Igepn,
    AgencyFamily::Peru,
    AgencyFamily::Other,
];

/// 地名后追加的机构括号标签，如 `[JMA]`、`[CN]`、`[CENC]`、`[四川地震局]`。
pub fn agency_bracket(agency: &str) -> String {
    if let Some(prov) = cea_pr_province(agency) {
        if prov.is_empty() {
            return "地方地震局".into();
        }
        if prov.ends_with("地震局") {
            return prov.to_string();
        }
        return format!("{prov}地震局");
    }
    match agency_family(agency) {
        AgencyFamily::Jma => "JMA".into(),
        AgencyFamily::Cea => "CN".into(),
        AgencyFamily::CeaPr => "省网".into(),
        AgencyFamily::Cenc => "CENC".into(),
        AgencyFamily::Gq => "GQ".into(),
        AgencyFamily::Usgs => "USGS".into(),
        AgencyFamily::Cwa => "CWA".into(),
        AgencyFamily::Kma => "KMA".into(),
        AgencyFamily::EarlyEst => "Early-est".into(),
        AgencyFamily::Ningxia => "宁夏地震局".into(),
        AgencyFamily::Yunnan => "云南地震局".into(),
        AgencyFamily::Shanxi => "山西地震局".into(),
        AgencyFamily::Beijing => "北京地震局".into(),
        AgencyFamily::Hko => "HKO".into(),
        AgencyFamily::Emsc => "EMSC".into(),
        AgencyFamily::Sa => "SA".into(),
        AgencyFamily::Gfz => "GFZ".into(),
        AgencyFamily::Bcsf => "BCSF".into(),
        AgencyFamily::Usp => "USP".into(),
        AgencyFamily::Ingv => "INGV".into(),
        AgencyFamily::Tmd => "TMD".into(),
        AgencyFamily::Mmd => "MMD".into(),
        AgencyFamily::Bmkg => "BMKG".into(),
        AgencyFamily::Geonet => "GeoNet".into(),
        AgencyFamily::Afad => "AFAD".into(),
        AgencyFamily::Ipma => "IPMA".into(),
        AgencyFamily::Noa => "NOA".into(),
        AgencyFamily::Sed => "SED".into(),
        AgencyFamily::Scsn => "SCSN".into(),
        AgencyFamily::Ipgp => "IPGP".into(),
        AgencyFamily::Infp => "INFP".into(),
        AgencyFamily::Isc => "ISC".into(),
        AgencyFamily::Knmi => "KNMI".into(),
        AgencyFamily::Ncedc => "NCEDC".into(),
        AgencyFamily::Lmu => "LMU".into(),
        AgencyFamily::Koeri => "KOERI".into(),
        AgencyFamily::Gsras => "GSRAS".into(),
        AgencyFamily::Csn => "CSN".into(),
        AgencyFamily::Phivolcs => "PHIVOLCS".into(),
        AgencyFamily::Ssn => "SSN".into(),
        AgencyFamily::Ga => "GA".into(),
        AgencyFamily::Igepn => "IGEPN".into(),
        AgencyFamily::Peru => "IGP".into(),
        AgencyFamily::Other => "其他".into(),
    }
}

/// 从 `cea-pr` / `cea-pr:四川` 解析省份；非 cea-pr 返回 `None`。
pub fn cea_pr_province(agency: &str) -> Option<&str> {
    let a = agency.trim();
    let lower = a.to_ascii_lowercase().replace('_', "-");
    if lower == "cea-pr" || lower == "wolfx-cea-pr" {
        return Some("");
    }
    for prefix in ["wolfx-cea-pr:", "wolfx-cea-pr/", "cea-pr:", "cea-pr/"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let start = a.len().saturating_sub(rest.len());
            return Some(a[start..].trim());
        }
    }
    None
}

/// 组装 cea-pr 的 agency 存储键（含省份时为 `cea-pr:四川`）。
pub fn cea_pr_agency_id(province: Option<&str>) -> String {
    match province.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => format!("cea-pr:{p}"),
        None => "cea-pr".into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgencyFamily {
    Jma,
    /// 中国地震预警网国网（cea），括号标签 `CN`
    Cea,
    /// 中国地震预警网省网（cea-pr），括号标签 `省网`
    CeaPr,
    /// 中国地震台网中心速报（cenc），括号标签 `CENC`
    Cenc,
    /// GlobalQuake（Pancakes gq）
    Gq,
    Usgs,
    Cwa,
    Kma,
    EarlyEst,
    Ningxia,
    Yunnan,
    Shanxi,
    Beijing,
    Hko,
    Emsc,
    Sa,
    Gfz,
    Bcsf,
    Usp,
    Ingv,
    Tmd,
    Mmd,
    Bmkg,
    Geonet,
    Afad,
    Ipma,
    Noa,
    Sed,
    Scsn,
    Ipgp,
    Infp,
    Isc,
    Knmi,
    Ncedc,
    Lmu,
    Koeri,
    Gsras,
    Csn,
    Phivolcs,
    Ssn,
    Ga,
    Igepn,
    Peru,
    Other,
}

impl AgencyFamily {
    pub fn as_filter_key(self) -> &'static str {
        match self {
            Self::Jma => "jma",
            Self::Cea => "cea",
            Self::CeaPr => "cea_pr",
            Self::Cenc => "cenc",
            Self::Gq => "gq",
            Self::Usgs => "usgs",
            Self::Cwa => "cwa",
            Self::Kma => "kma",
            Self::EarlyEst => "early_est",
            Self::Ningxia => "ningxia",
            Self::Yunnan => "yunnan",
            Self::Shanxi => "shanxi",
            Self::Beijing => "beijing",
            Self::Hko => "hko",
            Self::Emsc => "emsc",
            Self::Sa => "sa",
            Self::Gfz => "gfz",
            Self::Bcsf => "bcsf",
            Self::Usp => "usp",
            Self::Ingv => "ingv",
            Self::Tmd => "tmd",
            Self::Mmd => "mmd",
            Self::Bmkg => "bmkg",
            Self::Geonet => "geonet",
            Self::Afad => "afad",
            Self::Ipma => "ipma",
            Self::Noa => "noa",
            Self::Sed => "sed",
            Self::Scsn => "scsn",
            Self::Ipgp => "ipgp",
            Self::Infp => "infp",
            Self::Isc => "isc",
            Self::Knmi => "knmi",
            Self::Ncedc => "ncedc",
            Self::Lmu => "lmu",
            Self::Koeri => "koeri",
            Self::Gsras => "gsras",
            Self::Csn => "csn",
            Self::Phivolcs => "phivolcs",
            Self::Ssn => "ssn",
            Self::Ga => "ga",
            Self::Igepn => "igepn",
            Self::Peru => "peru",
            Self::Other => "other",
        }
    }

    /// 设置行右侧短标签（对齐 RhythmQuake 括号风格）。
    pub fn bracket_label(self) -> &'static str {
        match self {
            Self::Jma => "JMA",
            Self::Cea => "CN",
            Self::CeaPr => "省网",
            Self::Cenc => "CENC",
            Self::Gq => "GQ",
            Self::Usgs => "USGS",
            Self::Cwa => "CWA",
            Self::Kma => "KMA",
            Self::EarlyEst => "Early-est",
            Self::Ningxia => "宁夏",
            Self::Yunnan => "云南",
            Self::Shanxi => "山西",
            Self::Beijing => "北京",
            Self::Hko => "HKO",
            Self::Emsc => "EMSC",
            Self::Sa => "SA",
            Self::Gfz => "GFZ",
            Self::Bcsf => "BCSF",
            Self::Usp => "USP",
            Self::Ingv => "INGV",
            Self::Tmd => "TMD",
            Self::Mmd => "MMD",
            Self::Bmkg => "BMKG",
            Self::Geonet => "GeoNet",
            Self::Afad => "AFAD",
            Self::Ipma => "IPMA",
            Self::Noa => "NOA",
            Self::Sed => "SED",
            Self::Scsn => "SCSN",
            Self::Ipgp => "IPGP",
            Self::Infp => "INFP",
            Self::Isc => "ISC",
            Self::Knmi => "KNMI",
            Self::Ncedc => "NCEDC",
            Self::Lmu => "LMU",
            Self::Koeri => "KOERI",
            Self::Gsras => "GSRAS",
            Self::Csn => "CSN",
            Self::Phivolcs => "PHIVOLCS",
            Self::Ssn => "SSN",
            Self::Ga => "GA",
            Self::Igepn => "IGEPN",
            Self::Peru => "IGP",
            Self::Other => "其他",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Cea => "中国地震预警网",
            Self::CeaPr => "中国地震预警网省网",
            Self::Jma => "日本气象厅緊急地震速報",
            Self::Cenc => "中国地震台网 (CENC)",
            Self::Gq => "GlobalQuake 全球预警",
            Self::Usgs => "美国地质调查局 (USGS)",
            Self::Cwa => "台湾中央气象署预警",
            Self::Kma => "韩国气象厅预警",
            Self::EarlyEst => "Early-est快速定位系统",
            Self::Ningxia => "宁夏地震局",
            Self::Yunnan => "云南地震局",
            Self::Shanxi => "山西地震局",
            Self::Beijing => "北京地震局",
            Self::Hko => "香港天文台 (HKO)",
            Self::Emsc => "欧洲地中海地震中心",
            Self::Sa => "美国ShakeAlert预警",
            Self::Gfz => "德国地学研究中心",
            Self::Bcsf => "法国中央地震研究所",
            Self::Usp => "巴西圣保罗大学",
            Self::Ingv => "意大利国家地球物理与火山学研究所",
            Self::Tmd => "泰国气象局",
            Self::Mmd => "马来西亚气象局",
            Self::Bmkg => "印度尼西亚气象气候与地球物理局",
            Self::Geonet => "新西兰地球科学局",
            Self::Afad => "土耳其灾害与应急管理局",
            Self::Ipma => "葡萄牙气象局",
            Self::Noa => "希腊国家天文台",
            Self::Sed => "瑞士地震服务中心",
            Self::Scsn => "南加州地震数据中心",
            Self::Ipgp => "巴黎地球物理研究所",
            Self::Infp => "罗马尼亚国家地球物理研究所",
            Self::Isc => "国际地震中心",
            Self::Knmi => "荷兰皇家气象研究所",
            Self::Ncedc => "北加州地震数据中心",
            Self::Lmu => "慕尼黑大学地震服务",
            Self::Koeri => "坎迪利天文台地震研究所",
            Self::Gsras => "俄罗斯地球物理局",
            Self::Csn => "智利国家地震中心",
            Self::Phivolcs => "菲律宾火山与地震研究所",
            Self::Ssn => "墨西哥国家地震局",
            Self::Ga => "澳大利亚地质局",
            Self::Igepn => "厄瓜多尔地球物理研究所",
            Self::Peru => "秘鲁地球物理研究所",
            Self::Other => "未适配机构",
        }
    }
}

/// EEW 镜像源优先级：Jian > Wolfx > 其它（同事件合并时高者优先展示机构名）。
pub fn eew_feed_priority(agency: &str) -> u8 {
    let a = agency.trim().to_ascii_lowercase().replace('_', "-");
    if a.starts_with("wolfx") {
        return 2;
    }
    if a.contains("p2p") || a.contains("pancakes") {
        return 1;
    }
    // Jian 直连 / 中转（jma-eew、cea、cwa…）
    3
}

pub fn agency_family(agency: &str) -> AgencyFamily {
    let a = agency.trim().to_ascii_lowercase().replace('_', "-");

    if a.contains("early-est") || a.contains("earlyest") {
        return AgencyFamily::EarlyEst;
    }
    if a == "gq" || a.contains("globalquake") || a.starts_with("gq-") {
        return AgencyFamily::Gq;
    }
    if a.contains("usgs") {
        return AgencyFamily::Usgs;
    }
    if a.contains("cwa") {
        return AgencyFamily::Cwa;
    }
    if a.contains("kma") {
        return AgencyFamily::Kma;
    }
    if a.contains("ningxia") {
        return AgencyFamily::Ningxia;
    }
    if a.contains("yunnan") {
        return AgencyFamily::Yunnan;
    }
    if a.contains("shanxi") {
        return AgencyFamily::Shanxi;
    }
    if a.contains("beijing") {
        return AgencyFamily::Beijing;
    }
    if a.contains("hko") {
        return AgencyFamily::Hko;
    }
    if a.contains("emsc") || a.contains("euro") {
        return AgencyFamily::Emsc;
    }
    if a == "sa" || a.starts_with("sa-") || a.ends_with("-sa") {
        return AgencyFamily::Sa;
    }
    // CENC 速报（含 Wolfx）；勿与 CEA 预警混用
    if a == "cenc"
        || a.starts_with("cenc-")
        || a.contains("wolfx-cenc")
        || (a.contains("wolfx") && a.contains("cenc"))
    {
        return AgencyFamily::Cenc;
    }
    // 省网 cea-pr（须先于国网 cea 判定）
    if a == "cea-pr"
        || a.starts_with("cea-pr:")
        || a.starts_with("cea-pr/")
        || a.contains("wolfx-cea-pr")
        || a.contains("cea-pr")
    {
        return AgencyFamily::CeaPr;
    }
    // 国网 CEA 预警（Wolfx cenc_eew → wolfx-cea）
    if a == "cea"
        || a == "wolfx-cea"
        || a.starts_with("cea-")
        || a.starts_with("wolfx-cea")
        || a.contains("cea/")
    {
        return AgencyFamily::Cea;
    }
    if a.contains("jma") || a.contains("p2p") || a.contains("wolfx-jma") {
        return AgencyFamily::Jma;
    }
    if a.contains("gfz") || a.contains("geofon") {
        return AgencyFamily::Gfz;
    }
    if a.contains("bcsf") {
        return AgencyFamily::Bcsf;
    }
    if a.contains("usp") {
        return AgencyFamily::Usp;
    }
    if a.contains("ingv") {
        return AgencyFamily::Ingv;
    }
    if a == "tmd" || a.starts_with("tmd-") {
        return AgencyFamily::Tmd;
    }
    if a == "mmd" || a.starts_with("mmd-") {
        return AgencyFamily::Mmd;
    }
    if a.contains("bmkg") {
        return AgencyFamily::Bmkg;
    }
    if a.contains("geonet") {
        return AgencyFamily::Geonet;
    }
    if a.contains("afad") {
        return AgencyFamily::Afad;
    }
    if a.contains("ipma") {
        return AgencyFamily::Ipma;
    }
    if a == "noa" || a.starts_with("noa-") {
        return AgencyFamily::Noa;
    }
    if a == "sed" || a.starts_with("sed-") {
        return AgencyFamily::Sed;
    }
    if a.contains("scsn") || a.contains("scedc") {
        return AgencyFamily::Scsn;
    }
    if a.contains("ipgp") {
        return AgencyFamily::Ipgp;
    }
    if a.contains("infp") || a.contains("niep") {
        return AgencyFamily::Infp;
    }
    if a == "isc" || a.starts_with("isc-") {
        return AgencyFamily::Isc;
    }
    if a.contains("knmi") {
        return AgencyFamily::Knmi;
    }
    if a.contains("ncedc") {
        return AgencyFamily::Ncedc;
    }
    if a == "lmu" || a.starts_with("lmu-") {
        return AgencyFamily::Lmu;
    }
    if a.contains("koeri") {
        return AgencyFamily::Koeri;
    }
    if a.contains("gsras") {
        return AgencyFamily::Gsras;
    }
    if a == "csn" || a.starts_with("csn-") {
        return AgencyFamily::Csn;
    }
    if a.contains("phivolcs") {
        return AgencyFamily::Phivolcs;
    }
    if a == "ssn" || a.starts_with("ssn-") {
        return AgencyFamily::Ssn;
    }
    if a == "ga" || a.starts_with("ga-") || a.contains("geoscience-australia") {
        return AgencyFamily::Ga;
    }
    if a.contains("igepn") {
        return AgencyFamily::Igepn;
    }
    if a == "peru" || a.contains("igp") {
        return AgencyFamily::Peru;
    }
    AgencyFamily::Other
}

/// `马鲁古海北部 [JMA]` / `红原县 [四川地震局]` / `四川 [CENC]`
pub fn place_with_agency(place: &str, agency: &str) -> String {
    let tag = agency_bracket(agency);
    let bracketed = format!("[{tag}]");
    if place.contains(&bracketed) {
        place.to_string()
    } else {
        format!("{place} {bracketed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cea_is_cn_not_cenc() {
        assert_eq!(agency_bracket("cea"), "CN");
        assert_eq!(agency_family("cea"), AgencyFamily::Cea);
        assert_eq!(AgencyFamily::Cea.as_filter_key(), "cea");
        assert_eq!(place_with_agency("缅甸", "cea"), "缅甸 [CN]");
    }

    #[test]
    fn cenc_is_cenc_not_cn() {
        assert_eq!(agency_bracket("cenc"), "CENC");
        assert_eq!(agency_family("cenc"), AgencyFamily::Cenc);
        assert_eq!(AgencyFamily::Cenc.as_filter_key(), "cenc");
        assert_eq!(place_with_agency("宝兴县", "cenc"), "宝兴县 [CENC]");
        assert_eq!(agency_family("wolfx-cenc"), AgencyFamily::Cenc);
    }

    #[test]
    fn cea_pr_maps_to_province_bureau() {
        assert_eq!(agency_bracket("cea-pr:四川"), "四川地震局");
        assert_eq!(agency_bracket("cea-pr"), "地方地震局");
        assert_eq!(agency_family("cea-pr:云南"), AgencyFamily::CeaPr);
        assert_eq!(
            place_with_agency("阿坝州红原县", "cea-pr:四川"),
            "阿坝州红原县 [四川地震局]"
        );
        assert_eq!(cea_pr_agency_id(Some("四川")), "cea-pr:四川");
        assert_eq!(agency_family("wolfx-cea"), AgencyFamily::Cea);
        assert_eq!(agency_bracket("wolfx-cea"), "CN");
        assert_eq!(agency_bracket("wolfx-cea-pr:四川"), "四川地震局");
        assert_eq!(agency_family("wolfx-cea-pr:福建"), AgencyFamily::CeaPr);
        assert_eq!(agency_family("wolfx-cwa-eew"), AgencyFamily::Cwa);
        assert_eq!(AgencyFamily::CeaPr.bracket_label(), "省网");
        assert_eq!(AgencyFamily::Jma.display_name(), "日本气象厅緊急地震速報");
    }

    #[test]
    fn sa_is_shakealert_not_sichuan() {
        assert_eq!(agency_family("sa"), AgencyFamily::Sa);
        assert_eq!(AgencyFamily::Sa.display_name(), "美国ShakeAlert预警");
        assert_eq!(agency_bracket("sa"), "SA");
    }
}
