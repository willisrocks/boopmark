use askama::Template;
use axum::response::{Html, IntoResponse};

#[derive(Template)]
#[template(path = "legal/privacy.html")]
struct PrivacyPage;

#[derive(Template)]
#[template(path = "legal/support.html")]
struct SupportPage;

pub(crate) async fn privacy() -> impl IntoResponse {
    render(PrivacyPage)
}

pub(crate) async fn support() -> impl IntoResponse {
    render(SupportPage)
}

fn render(template: impl Template) -> axum::response::Response {
    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_page_names_mobile_data_and_service_providers() {
        let body = PrivacyPage.render().expect("privacy template should render");
        assert!(body.contains("Bookmark content"));
        assert!(body.contains("Anthropic"));
        assert!(body.contains("August 27, 2026"));
    }

    #[test]
    fn support_page_explains_connection_and_sharing() {
        let body = SupportPage.render().expect("support template should render");
        assert!(body.contains("Connect the iPhone app"));
        assert!(body.contains("Share Sheet"));
    }
}
