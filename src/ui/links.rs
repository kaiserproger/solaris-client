use crate::protocol::Result;
/// Components cannot open local files, execute scripts or embed credentials. The UI requires confirmation separately.
pub(super) fn safe_url(value: &str) -> Result<String> {
    if value.len() > 2048 || value.chars().any(char::is_control) {
        return Err("component_url_bound".into());
    }
    let url = reqwest::Url::parse(value).map_err(|_| "component_url_invalid")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("component_url_scheme_or_credentials".into());
    }
    Ok(url.into())
}
