use serde_json::Value;

pub(crate) const LATEST_URL: &str =
    "https://api.github.com/repos/warmUP-corp/warmup-companion/releases/latest";
pub(crate) const DOWNLOAD_PREFIX: &str =
    "https://github.com/warmUP-corp/warmup-companion/releases/download/";
pub(crate) const PAGE_PREFIX: &str = "https://github.com/warmUP-corp/warmup-companion/releases/";
pub(crate) const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn tag_version(json: &Value) -> Result<String, String> {
    Ok(json["tag_name"]
        .as_str()
        .ok_or("reply has no tag_name")?
        .trim_start_matches('v')
        .to_string())
}

pub(crate) fn asset(json: &Value, name: &str) -> Option<Value> {
    json["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"] == name)
        .cloned()
}

pub(crate) fn download_url(asset: &Value) -> Option<String> {
    asset["browser_download_url"]
        .as_str()
        .filter(|u| u.starts_with(DOWNLOAD_PREFIX))
        .map(str::to_string)
}

pub(crate) fn page_url(json: &Value) -> String {
    json["html_url"]
        .as_str()
        .filter(|u| u.starts_with(PAGE_PREFIX))
        .unwrap_or(PAGE_PREFIX)
        .to_string()
}

pub(crate) fn sidecar_sha(text: &str) -> Option<String> {
    text.split_whitespace().next().map(str::to_ascii_lowercase)
}

pub(crate) fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u32, u32, u32)> {
        let mut p = v.split('.').map(|s| s.parse().ok());
        Some((p.next()??, p.next()??, p.next()??))
    }
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

pub(crate) fn release_notes(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(
            |l| match l.strip_prefix("**").and_then(|r| r.split_once("**")) {
                Some((head, _)) => head.trim_end_matches('.').to_string(),
                None => l.split(". ").next().unwrap_or(l).replace('`', ""),
            },
        )
        .filter(|l| !l.is_empty())
        .take(3)
        .collect()
}
