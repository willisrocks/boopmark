-- migrations/007_add_user_role_and_deactivated_at.sql
CREATE TYPE user_role AS ENUM ('owner', 'admin', 'user');
ALTER TABLE users ADD COLUMN role user_role NOT NULL DEFAULT 'user';
ALTER TABLE users ADD COLUMN deactivated_at TIMESTAMPTZ;
