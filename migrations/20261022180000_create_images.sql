
CREATE TABLE images (
    id UUID PRIMARY KEY,
    file_name TEXT NOT NULL UNIQUE,
    original_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
