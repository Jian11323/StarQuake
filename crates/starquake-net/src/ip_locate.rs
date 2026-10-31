//! 公网 IP 归属定位：`GET https://api.sismotide.top/get/ip.json`（缺省查客户端 IP）。

use serde::Deserialize;

const IP_URL: &str = "https://api.sismotide.top/get/ip.json";
const UA: &str = concat!(
    "StarQuake/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Jian11323/StarQuake)"
);

#[derive(Debug, Clone)]
pub struct IpLocateOk {
    pub lat: f64,
    pub lon: f64,
    /// 地名摘要，如「中国·河南·郑州」；无则空串。
    pub location: String,
    pub ip: String,
}

#[derive(Debug, Deserialize)]
struct IpBody {
    ip: Option<String>,
    skipped: Option<bool>,
    lat: Option<f64>,
    lon: Option<f64>,
    location: Option<String>,
    error: Option<String>,
}

/// 按本机公网出口 IP 查询大致经纬度（城市级，仅供参考）。
pub fn locate_by_public_ip() -> Result<IpLocateOk, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(UA)
        .build()
        .map_err(|e| e.to_string())?;
    let url = crate::jian_endpoint::rewrite_api_url(IP_URL);
    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let body: IpBody =
        serde_json::from_str(&text).map_err(|e| format!("响应无法解析：{e}"))?;
    if body.skipped == Some(true) {
        return Err("当前出口为内网地址，无法 IP 定位".into());
    }
    if let Some(err) = body.error.filter(|s| !s.is_empty()) {
        return Err(err);
    }
    let lat = body.lat.ok_or_else(|| "归属结果无纬度".to_string())?;
    let lon = body.lon.ok_or_else(|| "归属结果无经度".to_string())?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err("经纬度超出有效范围".into());
    }
    Ok(IpLocateOk {
        lat,
        lon,
        location: body.location.unwrap_or_default(),
        ip: body.ip.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample_json() {
        let raw = r#"{"ip":"1.2.3.4","lat":34.757,"lon":113.649,"location":"中国·河南·郑州"}"#;
        let body: IpBody = serde_json::from_str(raw).unwrap();
        assert!((body.lat.unwrap() - 34.757).abs() < 1e-6);
        assert!((body.lon.unwrap() - 113.649).abs() < 1e-6);
    }
}
