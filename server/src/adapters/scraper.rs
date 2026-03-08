use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use crate::domain::ports::metadata::MetadataExtractor;
use ::scraper::{Html, Selector};
use url::Url;

#[derive(Clone)]
pub struct HtmlMetadataExtractor {
    client: reqwest::Client,
}

impl HtmlMetadataExtractor {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
        }
    }
}

impl MetadataExtractor for HtmlMetadataExtractor {
    async fn extract(&self, url_str: &str) -> Result<UrlMetadata, DomainError> {
        let parsed_url = Url::parse(url_str)
            .map_err(|e| DomainError::InvalidInput(format!("invalid URL: {e}")))?;

        let domain = parsed_url.host_str().map(|h| h.to_string());

        let resp = self
            .client
            .get(url_str)
            .send()
            .await
            .map_err(|e| DomainError::Internal(format!("fetch error: {e}")))?;
        let html = resp
            .text()
            .await
            .map_err(|e| DomainError::Internal(format!("read error: {e}")))?;

        let document = Html::parse_document(&html);

        let title = select_meta(&document, "og:title").or_else(|| {
            let sel = Selector::parse("title").ok()?;
            document
                .select(&sel)
                .next()
                .map(|el| el.text().collect::<String>())
        });

        let description = select_meta(&document, "og:description")
            .or_else(|| select_meta_name(&document, "description"));

        let image_url = select_meta(&document, "og:image").map(|img| resolve_url(url_str, &img));

        Ok(UrlMetadata {
            title,
            description,
            image_url,
            domain,
        })
    }
}

fn select_meta(document: &Html, property: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[property=\"{property}\"]")).ok()?;
    document
        .select(&selector)
        .next()?
        .value()
        .attr("content")
        .map(|s| s.to_string())
}

fn select_meta_name(document: &Html, name: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[name=\"{name}\"]")).ok()?;
    document
        .select(&selector)
        .next()?
        .value()
        .attr("content")
        .map(|s| s.to_string())
}

fn resolve_url(base: &str, relative: &str) -> String {
    if relative.starts_with("http") {
        return relative.to_string();
    }
    Url::parse(base)
        .and_then(|b| b.join(relative))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| relative.to_string())
}
