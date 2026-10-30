//! Pull company / role / location / salary out of a job posting page.
//!
//! Most job boards embed a schema.org `JobPosting` block as JSON-LD, which is the
//! reliable path. Failing that we fall back to Open Graph tags and the `<title>`.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    pub company: String,
    pub role: String,
    pub location: String,
    pub salary: String,
    /// The page title, kept so the caller has something to show if the fields above are empty.
    pub title: String,
    /// The link with tracking parameters removed and short links resolved.
    pub url: String,
}

/// Hosts that only ever redirect somewhere else; for these we adopt the destination URL.
const SHORTENERS: &[&str] = &["lnkd.in", "bit.ly", "t.co", "tinyurl.com", "goo.gl", "rb.gy", "is.gd", "buff.ly", "shorturl.at", "ow.ly", "cutt.ly", "rebrand.ly"];

/// Query parameters that only exist to track where a click came from.
/// Job identifiers such as `gh_jid`, `ashby_jid`, `jk`, `currentJobId` are deliberately absent.
const TRACKING_PARAMS: &[&str] = &[
    // generic analytics
    "fbclid", "gclid", "gclsrc", "dclid", "msclkid", "twclid", "ttclid", "igshid", "yclid", "wbraid", "gbraid",
    "_ga", "_gl", "mkt_tok", "mc_cid", "mc_eid", "ncid", "spm", "cmpid", "campaign", "campaign_id",
    "_branch_match_id", "_branch_referrer", "share", "share_id", "sharedid", "si",
    // referral / source
    "ref", "refid", "referer", "referrer", "ref_src", "ref_url", "source", "src", "origin", "via", "from",
    // LinkedIn
    "trackingid", "trk", "trkinfo", "trkemail", "lipi", "licu", "midtoken", "midsig", "eid", "otptoken",
    "position", "pagenum", "origintolandingjobpostings", "refinement", "lici",
    // Greenhouse / Lever / Workday / Indeed / Glassdoor
    "gh_src", "lever-source", "lever-source[]", "lever-origin", "lever-via", "tk", "vjs", "advn", "adid",
    "sjdu", "xkcb", "xpse", "jobsource", "utm", "s_cid", "sc_cid", "cid", "mkt", "mktg", "ao", "pos",
];
const TRACKING_PREFIXES: &[&str] = &["utm_", "mc_", "pk_", "piwik_", "hsa_", "_hs", "vero_", "oly_", "trk_", "ga_", "mtm_", "itm_", "sr_share"];

fn is_tracking_param(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    TRACKING_PARAMS.contains(&n.as_str()) || TRACKING_PREFIXES.iter().any(|p| n.starts_with(p))
}

/// Strip tracking parameters and the fragment from a URL. Unparseable input is returned trimmed.
pub fn clean_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw.trim()) else { return raw.trim().to_string() };
    url.set_fragment(None);
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
    let mut s = url.to_string();
    if s.ends_with('?') {
        s.pop();
    }
    s
}

fn host_of(url: &Url) -> &str {
    let h = url.host_str().unwrap_or("");
    h.strip_prefix("www.").unwrap_or(h)
}

/// Decide which URL to keep after following redirects: the destination for short links,
/// otherwise the original. Login walls are never adopted.
fn canonical_url(original: &str, final_url: &Url) -> String {
    let cleaned_original = clean_url(original);
    let Ok(orig) = Url::parse(&cleaned_original) else { return cleaned_original };
    let is_shortener = SHORTENERS.contains(&host_of(&orig));
    let path = final_url.path().to_ascii_lowercase();
    let looks_like_wall = ["authwall", "login", "signin", "sign-in", "checkpoint", "sso"].iter().any(|w| path.contains(w));
    if is_shortener && !looks_like_wall {
        clean_url(final_url.as_str())
    } else {
        cleaned_original
    }
}

static LD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<script[^>]*type\s*=\s*["']application/ld\+json["'][^>]*>(.*?)</script>"#).unwrap()
});
static META_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<meta\b([^>]*)>").unwrap());
static ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)([a-zA-Z:_-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).unwrap());
static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
static WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .timeout(Duration::from_secs(12))
        .build()
        .expect("reqwest client")
}

pub async fn fetch(client: &reqwest::Client, url: &str) -> Result<JobInfo, String> {
    let cleaned = clean_url(url);
    let resp = client.get(&cleaned).send().await.map_err(|e| format!("request failed: {e}"))?;
    let final_url = resp.url().clone();
    if !resp.status().is_success() {
        return Err(format!("page returned HTTP {}", resp.status().as_u16()));
    }
    let html = resp.text().await.map_err(|e| format!("could not read page: {e}"))?;
    let mut info = parse(&html, &final_url);
    info.url = canonical_url(url, &final_url);
    Ok(info)
}

pub fn parse(html: &str, url: &Url) -> JobInfo {
    let mut info = JobInfo::default();

    for cap in LD_RE.captures_iter(html) {
        if let Ok(v) = serde_json::from_str::<Value>(cap[1].trim()) {
            if let Some(jp) = find_job_posting(&v) {
                apply_job_posting(jp, &mut info);
                break;
            }
        }
    }

    let meta = meta_tags(html);
    let title = meta
        .get("og:title")
        .cloned()
        .or_else(|| TITLE_RE.captures(html).map(|c| decode_entities(&c[1])))
        .map(|t| tidy(&t))
        .unwrap_or_default();
    info.title = title.clone();

    if info.role.is_empty() || info.company.is_empty() {
        let (role, company) = split_title(&title);
        if info.role.is_empty() {
            info.role = role;
        }
        if info.company.is_empty() {
            info.company = company
                .or_else(|| meta.get("og:site_name").cloned())
                .unwrap_or_else(|| domain(url));
        }
    }
    info
}

/// JSON-LD can be a single object, an array, or an object with a `@graph` array.
fn find_job_posting(v: &Value) -> Option<&Value> {
    match v {
        Value::Array(items) => items.iter().find_map(find_job_posting),
        Value::Object(map) => {
            let is_job = match map.get("@type") {
                Some(Value::String(t)) => t == "JobPosting",
                Some(Value::Array(ts)) => ts.iter().any(|t| t == "JobPosting"),
                _ => false,
            };
            if is_job {
                Some(v)
            } else {
                map.get("@graph").and_then(find_job_posting)
            }
        }
        _ => None,
    }
}

fn apply_job_posting(jp: &Value, info: &mut JobInfo) {
    if let Some(t) = jp.get("title").and_then(Value::as_str) {
        info.role = tidy(t);
    }
    match jp.get("hiringOrganization") {
        Some(Value::String(s)) => info.company = tidy(s),
        Some(Value::Object(o)) => {
            if let Some(n) = o.get("name").and_then(Value::as_str) {
                info.company = tidy(n);
            }
        }
        _ => {}
    }

    let remote = matches!(jp.get("jobLocationType"), Some(Value::String(s)) if s.eq_ignore_ascii_case("TELECOMMUTE"));
    let mut places: Vec<String> = Vec::new();
    let locs: Vec<&Value> = match jp.get("jobLocation") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v) => vec![v],
        None => vec![],
    };
    for loc in locs {
        let addr = loc.get("address").unwrap_or(loc);
        let place = match addr {
            Value::String(s) => tidy(s),
            Value::Object(o) => ["addressLocality", "addressRegion", "addressCountry"]
                .iter()
                .filter_map(|k| o.get(*k).and_then(Value::as_str))
                .map(tidy)
                .filter(|s| !s.is_empty())
                .take(2)
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        };
        if !place.is_empty() && !places.contains(&place) {
            places.push(place);
        }
    }
    if remote {
        places.insert(0, "Remote".into());
    }
    places.truncate(3);
    info.location = places.join(" / ");

    if let Some(sal) = jp.get("baseSalary") {
        info.salary = format_salary(sal);
    }
}

fn format_salary(sal: &Value) -> String {
    let currency = sal.get("currency").and_then(Value::as_str).unwrap_or("");
    let val = sal.get("value").unwrap_or(sal);
    let (min, max, single, unit) = match val {
        Value::Object(o) => (
            o.get("minValue").and_then(as_f64),
            o.get("maxValue").and_then(as_f64),
            o.get("value").and_then(as_f64),
            o.get("unitText").and_then(Value::as_str).unwrap_or(""),
        ),
        other => (None, None, as_f64(other), ""),
    };
    let fmt = |n: f64| {
        if n >= 1000.0 {
            format!("{}k", (n / 1000.0).round() as i64)
        } else {
            format!("{}", n.round() as i64)
        }
    };
    let sym = match currency {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        _ => "",
    };
    let range = match (min, max, single) {
        (Some(a), Some(b), _) if a != b => format!("{sym}{}–{}", fmt(a), fmt(b)),
        (Some(a), _, _) | (_, Some(a), _) | (_, _, Some(a)) => format!("{sym}{}", fmt(a)),
        _ => return String::new(),
    };
    let unit = match unit.to_ascii_uppercase().as_str() {
        "YEAR" => "/yr",
        "MONTH" => "/mo",
        "HOUR" => "/hr",
        "WEEK" => "/wk",
        "DAY" => "/day",
        _ => "",
    };
    let cur = if sym.is_empty() && !currency.is_empty() { format!(" {currency}") } else { String::new() };
    format!("{range}{cur}{unit}")
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.replace([',', '$'], "").trim().parse().ok(),
        _ => None,
    }
}

fn meta_tags(html: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for tag in META_RE.captures_iter(html) {
        let mut key = None;
        let mut content = None;
        for a in ATTR_RE.captures_iter(&tag[1]) {
            let name = a[1].to_ascii_lowercase();
            let value = a.get(2).or(a.get(3)).or(a.get(4)).map(|m| m.as_str()).unwrap_or("");
            match name.as_str() {
                "property" | "name" => key = Some(value.to_ascii_lowercase()),
                "content" => content = Some(decode_entities(value)),
                _ => {}
            }
        }
        if let (Some(k), Some(c)) = (key, content) {
            out.entry(k).or_insert(c);
        }
    }
    out
}

/// Guess (role, company) from a page title like "Senior Engineer at Acme" or "Acme - Senior Engineer".
fn split_title(title: &str) -> (String, Option<String>) {
    let t = title.trim();
    if t.is_empty() {
        return (String::new(), None);
    }
    for sep in [" at ", " @ "] {
        if let Some((l, r)) = t.split_once(sep) {
            return (tidy(l), Some(strip_boilerplate(r)));
        }
    }
    for sep in [" - ", " – ", " — ", " | ", " · ", " • ", ": "] {
        if let Some((l, r)) = t.split_once(sep) {
            let r = strip_boilerplate(r.split(sep).next().unwrap_or(r));
            return (tidy(l), if r.is_empty() { None } else { Some(r) });
        }
    }
    (tidy(t), None)
}

fn strip_boilerplate(s: &str) -> String {
    let mut s = tidy(s);
    for word in ["Careers", "Jobs", "Job Board", "Job Openings", "Careers Page"] {
        if let Some(stripped) = s.strip_suffix(word) {
            s = tidy(stripped);
        }
    }
    s
}

fn domain(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    // "jobs.lever.co" -> "lever.co"; keep it short but recognizable.
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() > 2 { parts[parts.len() - 2..].join(".") } else { host.to_string() }
}

fn tidy(s: &str) -> String {
    WS_RE.replace_all(s.trim(), " ").into_owned()
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url() -> Url {
        Url::parse("https://boards.greenhouse.io/acme/jobs/123").unwrap()
    }

    #[test]
    fn strips_tracking_params() {
        assert_eq!(
            clean_url("https://www.linkedin.com/jobs/view/4012345678/?refId=abc123&trackingId=XYZ%3D%3D&trk=public_jobs_topcard-title&utm_source=share"),
            "https://www.linkedin.com/jobs/view/4012345678/"
        );
        assert_eq!(
            clean_url("https://boards.greenhouse.io/acme/jobs/555?gh_jid=555&gh_src=abcd1234"),
            "https://boards.greenhouse.io/acme/jobs/555?gh_jid=555"
        );
        assert_eq!(
            clean_url("https://www.indeed.com/viewjob?jk=deadbeef&from=shareddesktop&tk=1a2b#top"),
            "https://www.indeed.com/viewjob?jk=deadbeef"
        );
        assert_eq!(clean_url("https://jobs.ashbyhq.com/acme/1234?ashby_jid=1234"), "https://jobs.ashbyhq.com/acme/1234?ashby_jid=1234");
        assert_eq!(clean_url("  not a url "), "not a url");
    }

    #[test]
    fn short_links_adopt_destination_but_not_login_walls() {
        let dest = Url::parse("https://www.linkedin.com/jobs/view/42/?trk=x").unwrap();
        assert_eq!(canonical_url("https://lnkd.in/abc", &dest), "https://www.linkedin.com/jobs/view/42/");
        let wall = Url::parse("https://www.linkedin.com/authwall?sessionRedirect=x").unwrap();
        assert_eq!(canonical_url("https://lnkd.in/abc", &wall), "https://lnkd.in/abc");
        // A normal link that redirects keeps its own (cleaned) form.
        assert_eq!(canonical_url("https://acme.com/jobs/1?utm_medium=email", &dest), "https://acme.com/jobs/1");
    }

    #[test]
    fn parses_json_ld() {
        let html = r##"<html><head><title>Job</title>
        <script type="application/ld+json">{"@context":"https://schema.org","@type":"JobPosting",
          "title":"Senior Backend Engineer","hiringOrganization":{"@type":"Organization","name":"Acme Corp"},
          "jobLocationType":"TELECOMMUTE",
          "jobLocation":[{"@type":"Place","address":{"addressLocality":"Austin","addressRegion":"TX"}}],
          "baseSalary":{"@type":"MonetaryAmount","currency":"USD","value":{"@type":"QuantitativeValue","minValue":150000,"maxValue":190000,"unitText":"YEAR"}}}
        </script></head><body></body></html>"##;
        let info = parse(html, &url());
        assert_eq!(info.role, "Senior Backend Engineer");
        assert_eq!(info.company, "Acme Corp");
        assert_eq!(info.location, "Remote / Austin, TX");
        assert_eq!(info.salary, "$150k–190k/yr");
    }

    #[test]
    fn falls_back_to_og_and_title() {
        let html = r#"<html><head><meta content="Staff Engineer at Globex" property="og:title">
        <meta property="og:site_name" content="Globex Careers"><title>ignored</title></head></html>"#;
        let info = parse(html, &url());
        assert_eq!(info.role, "Staff Engineer");
        assert_eq!(info.company, "Globex");
    }

    #[test]
    fn falls_back_to_domain() {
        let html = "<html><head><title>Platform Engineer</title></head></html>";
        let info = parse(html, &url());
        assert_eq!(info.role, "Platform Engineer");
        assert_eq!(info.company, "greenhouse.io");
    }

    #[test]
    fn graph_wrapped_posting_and_hyphen_title() {
        let html = r##"<script type="application/ld+json">{"@graph":[{"@type":"WebSite"},{"@type":"JobPosting","title":"Data Scientist","hiringOrganization":"Initech"}]}</script>
        <title>Data Scientist - Initech - Jobs</title>"##;
        let info = parse(html, &url());
        assert_eq!(info.role, "Data Scientist");
        assert_eq!(info.company, "Initech");
        assert_eq!(split_title("Data Scientist - Initech - Jobs"), ("Data Scientist".into(), Some("Initech".into())));
    }
}
