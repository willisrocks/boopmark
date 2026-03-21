-- migrations/008_create_invites.sql
CREATE TABLE invites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token TEXT UNIQUE NOT NULL,
    email TEXT,
    created_by UUID NOT NULL REFERENCES users(id),
    claimed_by UUID REFERENCES users(id),
    revoked_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
