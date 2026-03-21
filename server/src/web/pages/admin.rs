use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::Form;
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::user::UserRole;
use crate::web::extractors::AdminUser;
use crate::web::pages::shared::UserView;
use crate::web::state::AppState;

struct InviteView {
    id: String,
    email: Option<String>,
    created_by_name: String,
    status: String,
    is_pending: bool,
    invite_url: String,
}

struct AdminUserView {
    id: String,
    email: String,
    name: Option<String>,
    image: Option<String>,
    email_initial: String,
    role: String,
    is_owner: bool,
    is_self: bool,
    created_at: String,
    is_active: bool,
}

#[derive(Template)]
#[template(path = "admin/index.html")]
struct AdminPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    invites: Vec<InviteView>,
    users: Vec<AdminUserView>,
}

#[derive(Template)]
#[template(path = "admin/invite_list.html")]
struct InviteListFragment {
    invites: Vec<InviteView>,
}

#[derive(Template)]
#[template(path = "admin/user_list.html")]
struct UserListFragment {
    users: Vec<AdminUserView>,
}

#[derive(Deserialize)]
pub struct CreateInviteForm {
    email: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateRoleForm {
    role: String,
}

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn build_invite_views(state: &AppState) -> Vec<InviteView> {
    let invites = state.invites.list_invites().await.unwrap_or_default();
    let users = state.auth.list_users().await.unwrap_or_default();

    invites
        .into_iter()
        .map(|inv| {
            let created_by_name = users
                .iter()
                .find(|u| u.id == inv.created_by)
                .and_then(|u| u.name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let status = inv.status().to_string();
            let is_pending = inv.is_pending();
            let invite_url = format!("{}/invite/{}", state.config.app_url, inv.token);
            InviteView {
                id: inv.id.to_string(),
                email: inv.email,
                created_by_name,
                status,
                is_pending,
                invite_url,
            }
        })
        .collect()
}

async fn build_user_views(state: &AppState, current_user_id: Uuid) -> Vec<AdminUserView> {
    let users = state.auth.list_users().await.unwrap_or_default();
    users
        .into_iter()
        .map(|u| {
            let email_initial = u.email.chars().next().unwrap_or('?').to_string();
            let is_owner = u.is_owner();
            let is_self = u.id == current_user_id;
            let is_active = u.is_active();
            let role = u.role.as_str().to_string();
            let created_at = u.created_at.format("%b %d, %Y").to_string();
            AdminUserView {
                id: u.id.to_string(),
                email: u.email,
                name: u.name,
                image: u.image,
                email_initial,
                role,
                is_owner,
                is_self,
                created_at,
                is_active,
            }
        })
        .collect()
}

pub async fn admin_page(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> axum::response::Response {
    let current_user_id = user.id;
    let invites = build_invite_views(&state).await;
    let users = build_user_views(&state, current_user_id).await;

    render(&AdminPage {
        user: Some(user.into()),
        header_shows_bookmark_actions: false,
        invites,
        users,
    })
}

pub async fn create_invite(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Form(form): Form<CreateInviteForm>,
) -> axum::response::Response {
    let email = form.email.filter(|e| !e.trim().is_empty());
    match state.invites.create_invite(user.id, email).await {
        Ok(_) => {
            let invites = build_invite_views(&state).await;
            render(&InviteListFragment { invites })
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn revoke_invite(
    State(state): State<AppState>,
    AdminUser(_user): AdminUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    match state.invites.revoke_invite(id).await {
        Ok(()) => {
            let invites = build_invite_views(&state).await;
            render(&InviteListFragment { invites })
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn update_user_role(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
    Form(form): Form<UpdateRoleForm>,
) -> axum::response::Response {
    // Parse the requested role
    let new_role = match form.role.as_str() {
        "admin" => UserRole::Admin,
        "user" => UserRole::User,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Cannot change own role
    if id == admin.id {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Look up target user
    let target = match state.auth.find_user_by_id(id).await {
        Ok(u) => u,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    // Cannot change owner's role
    if target.is_owner() {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Admin (non-owner) can only demote admin -> user, not promote user -> admin
    if !admin.is_owner() && new_role == UserRole::Admin {
        return StatusCode::FORBIDDEN.into_response();
    }

    match state.auth.update_user_role(id, new_role).await {
        Ok(()) => {
            let users = build_user_views(&state, admin.id).await;
            render(&UserListFragment { users })
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn deactivate_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    // Cannot deactivate self
    if id == admin.id {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Look up target user
    let target = match state.auth.find_user_by_id(id).await {
        Ok(u) => u,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    // Cannot deactivate owner
    if target.is_owner() {
        return StatusCode::FORBIDDEN.into_response();
    }

    match state.auth.deactivate_user(id).await {
        Ok(()) => {
            let users = build_user_views(&state, admin.id).await;
            render(&UserListFragment { users })
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
