-- Optional password hash for local auth (ENABLE_LOCAL_AUTH=1).
-- NULL for users who authenticate via Google OAuth only.
ALTER TABLE users ADD COLUMN password_hash TEXT;
