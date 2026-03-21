pub(crate) struct UserView {
    pub(crate) email: String,
    pub(crate) display_name: String,
    pub(crate) email_initial: String,
    pub(crate) image: Option<String>,
    pub(crate) is_admin_or_owner: bool,
}

impl From<crate::domain::user::User> for UserView {
    fn from(u: crate::domain::user::User) -> Self {
        let email_initial = u.email.chars().next().unwrap_or('?').to_string();
        let display_name = u.name.clone().unwrap_or_default();
        let is_admin_or_owner = u.is_admin_or_owner();
        Self {
            email: u.email,
            display_name,
            email_initial,
            image: u.image,
            is_admin_or_owner,
        }
    }
}
