use askama::Template;
use axum::Form;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use image::GenericImageView;
use serde::Deserialize;
use std::io::Cursor;
use uuid::Uuid;

use crate::domain::bookmark::{
    Bookmark, BookmarkFilter, BookmarkSort, CreateBookmark, UpdateBookmark,
};
use crate::domain::error::DomainError;
use crate::web::extractors::AuthUser;
use crate::web::middleware::auth::is_htmx;
use crate::web::pages::shared::UserView;
use crate::web::state::{AppState, Bookmarks};

/// Uploaded overrides are deliberately bounded before decoding. The body
/// limit on the route is slightly larger to allow multipart field overhead.
pub(crate) const MAX_IMAGE_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 10_000;
const MAX_IMAGE_PIXELS: u64 = 20_000_000;
const OVERRIDE_WIDTH: u32 = 1_200;
const OVERRIDE_HEIGHT: u32 = 630;

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

macro_rules! with_bookmarks {
    ($bookmarks:expr, $svc:ident => $body:expr) => {
        match $bookmarks {
            Bookmarks::Local($svc) => $body,
            Bookmarks::S3($svc) => $body,
        }
    };
}

/// Pre-computed view of a bookmark for templates.
struct BookmarkView {
    id: Uuid,
    url: String,
    title: Option<String>,
    description: Option<String>,
    image_url: Option<String>,
    tags: Vec<String>,
    created_at_display: String,
}

impl From<Bookmark> for BookmarkView {
    fn from(b: Bookmark) -> Self {
        let image_url = b.effective_image_url().map(str::to_string);
        Self {
            id: b.id,
            url: b.url,
            title: b.title,
            description: b.description,
            image_url,
            tags: b.tags,
            created_at_display: b.created_at.format("%b %d, %Y").to_string(),
        }
    }
}

/// Tag with pre-computed active state for the filter bar.
struct TagView {
    name: String,
    active: bool,
}

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn error_response(e: DomainError) -> axum::response::Response {
    let status = match &e {
        DomainError::NotFound => StatusCode::NOT_FOUND,
        DomainError::Unauthorized => StatusCode::UNAUTHORIZED,
        DomainError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, e.to_string()).into_response()
}

#[derive(Template)]
#[template(path = "bookmarks/grid.html")]
struct GridPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    bookmarks: Vec<BookmarkView>,
    filter_tags: Vec<TagView>,
    sort: String,
    suggest_title: String,
    suggest_description: String,
    #[allow(dead_code)]
    suggest_preview_image_url: Option<String>,
    suggest_tags: String,
}

#[derive(Template)]
#[template(path = "bookmarks/list_with_filters.html")]
struct BookmarkListWithFilters {
    bookmarks: Vec<BookmarkView>,
    filter_tags: Vec<TagView>,
    sort: String,
}

#[derive(Template)]
#[template(path = "bookmarks/card.html")]
struct BookmarkCard {
    bookmark: BookmarkView,
}

#[derive(Template)]
#[template(path = "bookmarks/add_modal_suggest_fields.html")]
struct SuggestFields {
    suggest_title: String,
    suggest_description: String,
    #[allow(dead_code)]
    suggest_preview_image_url: Option<String>,
    suggest_tags: String,
}

#[derive(Template)]
#[template(path = "bookmarks/edit_modal.html")]
struct EditModal {
    bookmark_id: Uuid,
    image_url: Option<String>,
    has_override: bool,
    suggest_title: String,
    suggest_description: String,
    suggest_tags: String,
    has_llm: bool,
}

#[derive(Template)]
#[template(path = "bookmarks/edit_suggest_fields.html")]
struct EditSuggestFields {
    suggest_title: String,
    suggest_description: String,
    suggest_tags: String,
}

#[derive(Deserialize)]
pub struct ListQuery {
    search: Option<String>,
    tags: Option<String>,
    sort: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> axum::response::Response {
    let active_tags: Vec<String> = query
        .tags
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let sort_str = query.sort.clone().unwrap_or_else(|| "newest".into());
    let sort = match sort_str.as_str() {
        "oldest" => BookmarkSort::Oldest,
        "title" => BookmarkSort::Title,
        "domain" => BookmarkSort::Domain,
        _ => BookmarkSort::Newest,
    };

    let filter = BookmarkFilter {
        search: query.search.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }),
        tags: if active_tags.is_empty() {
            None
        } else {
            Some(active_tags.clone())
        },
        sort: Some(sort),
        ..Default::default()
    };

    let bookmarks = with_bookmarks!(&state.bookmarks, svc => svc.list(user.id, filter).await)
        .unwrap_or_default();

    let bookmark_views: Vec<BookmarkView> = bookmarks.into_iter().map(Into::into).collect();

    // Query all distinct tags for the filter bar (used by both HTMX and full-page paths).
    let all_tag_names = with_bookmarks!(&state.bookmarks, svc =>
        svc.all_tags(user.id).await
    )
    .unwrap_or_default();
    let filter_tags: Vec<TagView> = all_tag_names
        .into_iter()
        .map(|name| {
            let active = active_tags.contains(&name);
            TagView { name, active }
        })
        .collect();

    if is_htmx(&headers) {
        render(&BookmarkListWithFilters {
            bookmarks: bookmark_views,
            filter_tags,
            sort: sort_str,
        })
    } else {
        render(&GridPage {
            user: Some(user.into()),
            header_shows_bookmark_actions: true,
            bookmarks: bookmark_views,
            filter_tags,
            sort: sort_str,
            suggest_title: String::new(),
            suggest_description: String::new(),
            suggest_preview_image_url: None,
            suggest_tags: String::new(),
        })
    }
}

#[derive(Deserialize)]
pub struct CreateForm {
    url: String,
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

#[derive(Deserialize)]
pub struct SuggestForm {
    url: String,
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateForm>,
) -> axum::response::Response {
    let tags = form
        .tags_input
        .filter(|t| !t.is_empty())
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect());

    let input = CreateBookmark {
        url: form.url,
        title: form.title.filter(|t| !t.is_empty()),
        description: form.description.filter(|d| !d.is_empty()),
        image_url: None,
        domain: None,
        tags,
    };

    match with_bookmarks!(&state.bookmarks, svc => svc.create(user.id, input).await) {
        Ok(bookmark) => render(&BookmarkCard {
            bookmark: bookmark.into(),
        }),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn suggest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SuggestForm>,
) -> axum::response::Response {
    let result = state.enrichment.suggest(user.id, &form.url, None).await;

    // Preserve user-typed tags; only use enrichment tags if user hasn't typed any
    let user_tags = form.tags_input.and_then(non_empty);
    let suggest_tags = user_tags.unwrap_or_else(|| {
        if result.tags.is_empty() {
            String::new()
        } else {
            result.tags.join(", ")
        }
    });

    render(&SuggestFields {
        suggest_title: fill_if_blank(form.title, result.title),
        suggest_description: fill_if_blank(form.description, result.description),
        suggest_preview_image_url: result.image_url,
        suggest_tags,
    })
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    match with_bookmarks!(&state.bookmarks, svc => svc.delete(id, user.id).await) {
        Ok(()) => Html("").into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

pub async fn edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    let bookmark = match with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        Ok(b) => b,
        Err(e) => return error_response(e),
    };

    let has_llm = state
        .settings
        .get_decrypted_api_key(user.id)
        .await
        .ok()
        .flatten()
        .is_some();

    render(&EditModal {
        bookmark_id: bookmark.id,
        image_url: bookmark.effective_image_url().map(str::to_string),
        has_override: bookmark.override_image_url.is_some(),
        suggest_title: bookmark.title.unwrap_or_default(),
        suggest_description: bookmark.description.unwrap_or_default(),
        suggest_tags: bookmark.tags.join(", "),
        has_llm,
    })
}

#[derive(Debug)]
struct ImageOverrideUpload {
    bytes: Vec<u8>,
    focal_x: f32,
    focal_y: f32,
}

/// Parse the small multipart form used by the image override widget. Keeping
/// this independent from the image decoder makes it easy to reject malformed
/// requests before doing any expensive work.
async fn parse_image_override(
    mut multipart: Multipart,
) -> Result<ImageOverrideUpload, DomainError> {
    let mut bytes = None;
    let mut focal_x = 0.5;
    let mut focal_y = 0.5;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| DomainError::InvalidInput(format!("invalid multipart upload: {error}")))?
    {
        let name = field.name().unwrap_or_default();
        match name {
            "image" | "file" => {
                if bytes.is_some() {
                    return Err(DomainError::InvalidInput(
                        "only one image may be uploaded".to_string(),
                    ));
                }
                let data = field.bytes().await.map_err(|error| {
                    DomainError::InvalidInput(format!("could not read image upload: {error}"))
                })?;
                if data.is_empty() {
                    return Err(DomainError::InvalidInput(
                        "image upload is empty".to_string(),
                    ));
                }
                if data.len() > MAX_IMAGE_UPLOAD_BYTES {
                    return Err(DomainError::InvalidInput(format!(
                        "image upload exceeds the {} MiB limit",
                        MAX_IMAGE_UPLOAD_BYTES / (1024 * 1024)
                    )));
                }
                bytes = Some(data.to_vec());
            }
            "focal_x" => {
                let value = field.text().await.map_err(|error| {
                    DomainError::InvalidInput(format!("invalid focal point: {error}"))
                })?;
                focal_x = parse_focal_point(&value, "focal_x")?;
            }
            "focal_y" => {
                let value = field.text().await.map_err(|error| {
                    DomainError::InvalidInput(format!("invalid focal point: {error}"))
                })?;
                focal_y = parse_focal_point(&value, "focal_y")?;
            }
            _ => {
                // Ignore browser-added fields so the endpoint remains
                // forwards-compatible with the preview widget.
            }
        }
    }

    let bytes = bytes.ok_or_else(|| DomainError::InvalidInput("image is required".to_string()))?;
    Ok(ImageOverrideUpload {
        bytes,
        focal_x,
        focal_y,
    })
}

fn parse_focal_point(value: &str, name: &str) -> Result<f32, DomainError> {
    let parsed = value
        .trim()
        .parse::<f32>()
        .map_err(|_| DomainError::InvalidInput(format!("{name} must be a number from 0 to 1")))?;
    if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
        return Err(DomainError::InvalidInput(format!(
            "{name} must be a number from 0 to 1"
        )));
    }
    Ok(parsed)
}

/// Decode, crop around the requested focal point, and re-encode every
/// override into the same exact card dimensions. The browser preview is only
/// a convenience; these bytes are the authoritative representation stored.
fn process_image_override(
    bytes: &[u8],
    focal_x: f32,
    focal_y: f32,
) -> Result<Vec<u8>, DomainError> {
    // Read the header first so a tiny compressed payload cannot expand into an
    // unbounded bitmap before our decoded-size guard runs.
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| DomainError::InvalidInput(format!("could not identify image: {error}")))?;
    let (header_width, header_height) = reader.into_dimensions().map_err(|error| {
        DomainError::InvalidInput(format!("could not read image dimensions: {error}"))
    })?;
    validate_image_dimensions(header_width, header_height)?;

    let image = image::load_from_memory(bytes)
        .map_err(|error| DomainError::InvalidInput(format!("could not decode image: {error}")))?;
    let (width, height) = image.dimensions();
    validate_image_dimensions(width, height)?;

    let (crop_x, crop_y, crop_width, crop_height) = crop_rect(width, height, focal_x, focal_y);
    let cropped = image.crop_imm(crop_x, crop_y, crop_width, crop_height);
    let resized = cropped.resize_exact(
        OVERRIDE_WIDTH,
        OVERRIDE_HEIGHT,
        image::imageops::FilterType::Lanczos3,
    );

    let mut output = Cursor::new(Vec::new());
    resized
        .write_to(&mut output, image::ImageFormat::Jpeg)
        .map_err(|error| DomainError::Internal(format!("could not encode image: {error}")))?;
    Ok(output.into_inner())
}

fn validate_image_dimensions(width: u32, height: u32) -> Result<(), DomainError> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || pixels > MAX_IMAGE_PIXELS
    {
        return Err(DomainError::InvalidInput(format!(
            "decoded image dimensions must be at most {MAX_IMAGE_DIMENSION}x{MAX_IMAGE_DIMENSION} and {MAX_IMAGE_PIXELS} pixels"
        )));
    }
    Ok(())
}

fn crop_rect(width: u32, height: u32, focal_x: f32, focal_y: f32) -> (u32, u32, u32, u32) {
    // Use integer arithmetic for the crop dimensions so the crop remains
    // deterministic across platforms. The final resize is always exact.
    let target_width = u64::from(height) * u64::from(OVERRIDE_WIDTH) / u64::from(OVERRIDE_HEIGHT);
    let target_height = u64::from(width) * u64::from(OVERRIDE_HEIGHT) / u64::from(OVERRIDE_WIDTH);
    let (crop_width, crop_height) = if u64::from(width) * u64::from(OVERRIDE_HEIGHT)
        >= u64::from(height) * u64::from(OVERRIDE_WIDTH)
    {
        (target_width.min(u64::from(width)) as u32, height)
    } else {
        (width, target_height.min(u64::from(height)) as u32)
    };

    let max_x = width.saturating_sub(crop_width);
    let max_y = height.saturating_sub(crop_height);
    let crop_x = ((max_x as f32) * focal_x).round() as u32;
    let crop_y = ((max_y as f32) * focal_y).round() as u32;
    (
        crop_x.min(max_x),
        crop_y.min(max_y),
        crop_width,
        crop_height,
    )
}

fn image_response(bookmark: Bookmark) -> axum::response::Response {
    render(&BookmarkCard {
        bookmark: bookmark.into(),
    })
}

pub async fn upload_image(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    multipart: Multipart,
) -> axum::response::Response {
    if let Err(error) = with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        return error_response(error);
    }
    let upload = match parse_image_override(multipart).await {
        Ok(upload) => upload,
        Err(error) => return error_response(error),
    };
    let _processing_permit = match state.image_processing_slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "image processor is busy; try again shortly",
            )
                .into_response();
        }
    };
    let focal_x = upload.focal_x;
    let focal_y = upload.focal_y;
    let processed = match tokio::task::spawn_blocking(move || {
        process_image_override(&upload.bytes, focal_x, focal_y)
    })
    .await
    {
        Ok(Ok(processed)) => processed,
        Ok(Err(error)) => return error_response(error),
        Err(error) => {
            return error_response(DomainError::Internal(format!(
                "image processing task failed: {error}"
            )));
        }
    };

    let update_result = with_bookmarks!(&state.bookmarks, svc => {
        svc.replace_image_override(id, user.id, processed).await
    });
    if let Err(error) = update_result {
        return error_response(error);
    }

    match with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        Ok(bookmark) => image_response(bookmark),
        Err(error) => error_response(error),
    }
}

pub async fn remove_image(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    if let Err(error) = with_bookmarks!(&state.bookmarks, svc => {
        svc.remove_image_override(id, user.id).await
    }) {
        return error_response(error);
    }
    match with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        Ok(bookmark) => image_response(bookmark),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
pub struct EditForm {
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

/// Separate form struct for the edit-suggest endpoint.
/// Unlike `SuggestForm` (used by the add flow), this does NOT include a `url`
/// field because the edit modal form has no URL input -- the URL is fetched
/// from the database by bookmark ID.
#[derive(Deserialize)]
pub struct EditSuggestForm {
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Form(form): Form<EditForm>,
) -> axum::response::Response {
    let tags = form.tags_input.filter(|t| !t.is_empty()).map(|t| {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    // Pass all three fields as Some(...) so the user can clear them.
    //
    // Title & description: the SQL uses
    //   CASE WHEN $n = '' THEN NULL ELSE COALESCE($n, col) END
    // so an empty string clears the field to NULL (matching never-set
    // semantics), a non-empty string updates it, and None (NULL) keeps
    // the old value.
    //
    // Tags: the SQL uses COALESCE($5, tags), so Some(vec![]) clears
    // the array to '{}' and None keeps the old tags.
    let input = UpdateBookmark {
        title: form.title,
        description: form.description,
        tags: Some(tags.unwrap_or_default()),
    };

    match with_bookmarks!(&state.bookmarks, svc => svc.update(id, user.id, input).await) {
        Ok(bookmark) => render(&BookmarkCard {
            bookmark: bookmark.into(),
        }),
        Err(e) => error_response(e),
    }
}

pub async fn edit_suggest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Form(form): Form<EditSuggestForm>,
) -> axum::response::Response {
    // Get the bookmark to find its URL
    let bookmark = match with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        Ok(b) => b,
        Err(e) => return error_response(e),
    };

    // Get existing tags with counts for LLM context
    let existing_tags = with_bookmarks!(&state.bookmarks, svc =>
        svc.tags_with_counts(user.id).await
    )
    .ok();

    let result = state
        .enrichment
        .suggest(user.id, &bookmark.url, existing_tags)
        .await;

    // For edit suggest, always prefer enrichment suggestions over current
    // form values. The user explicitly asked for suggestions, so we replace
    // all fields. Fall back to current form values only if no suggestion exists.
    let suggest_tags = if !result.tags.is_empty() {
        result.tags.join(", ")
    } else {
        form.tags_input.and_then(non_empty).unwrap_or_default()
    };

    let suggest_title = result
        .title
        .and_then(non_empty)
        .unwrap_or_else(|| form.title.and_then(non_empty).unwrap_or_default());

    let suggest_description = result
        .description
        .and_then(non_empty)
        .unwrap_or_else(|| form.description.and_then(non_empty).unwrap_or_default());

    render(&EditSuggestFields {
        suggest_title,
        suggest_description,
        suggest_tags,
    })
}

fn fill_if_blank(current: Option<String>, suggested: Option<String>) -> String {
    current
        .and_then(non_empty)
        .or_else(|| suggested.and_then(non_empty))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 255) as u8, (y % 255) as u8, 80])
        }));
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, ImageFormat::Png).unwrap();
        output.into_inner()
    }

    #[test]
    fn override_is_decoded_cropped_and_resized_to_card_dimensions() {
        let output = process_image_override(&png_bytes(2_000, 1_000), 0.8, 0.2).unwrap();
        let decoded = image::load_from_memory(&output).unwrap();
        assert_eq!(decoded.dimensions(), (OVERRIDE_WIDTH, OVERRIDE_HEIGHT));
        assert!(output.starts_with(&[0xff, 0xd8, 0xff]));
    }

    #[test]
    fn crop_focal_point_moves_crop_within_source_bounds() {
        let left = crop_rect(2_000, 1_000, 0.0, 0.5);
        let right = crop_rect(2_000, 1_000, 1.0, 0.5);
        assert_eq!(left.0, 0);
        assert_eq!(
            right.0,
            right.2.max(1).saturating_sub(right.2) + (2_000 - right.2)
        );
        assert!(right.0 > left.0);
        assert_eq!(left.1, right.1);
    }

    #[test]
    fn invalid_bytes_are_rejected_before_storage() {
        let error = process_image_override(b"not an image", 0.5, 0.5).unwrap_err();
        assert!(matches!(error, DomainError::InvalidInput(message) if message.contains("image")));
    }

    #[test]
    fn oversized_decoded_dimensions_are_rejected() {
        let error =
            process_image_override(&png_bytes(MAX_IMAGE_DIMENSION + 1, 1), 0.5, 0.5).unwrap_err();
        assert!(
            matches!(error, DomainError::InvalidInput(message) if message.contains("dimensions"))
        );
    }

    #[test]
    fn focal_point_values_are_strictly_bounded() {
        assert!(parse_focal_point("0", "focal_x").is_ok());
        assert!(parse_focal_point("1", "focal_x").is_ok());
        assert!(parse_focal_point("-0.01", "focal_x").is_err());
        assert!(parse_focal_point("1.01", "focal_x").is_err());
        assert!(parse_focal_point("NaN", "focal_x").is_err());
    }
}
