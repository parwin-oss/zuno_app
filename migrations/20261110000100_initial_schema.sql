CREATE TABLE users (
id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

username VARCHAR(30) NOT NULL,
email VARCHAR(255) NOT NULL,
password_hash TEXT NOT NULL,

display_name VARCHAR(100),
avatar_url TEXT,
bio VARCHAR(500),

is_active BOOLEAN NOT NULL DEFAULT TRUE,

created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

CONSTRAINT users_username_length
CHECK (char_length(username) >= 3),

CONSTRAINT users_email_not_empty
CHECK (char_length(email) > 0)
);

CREATE UNIQUE INDEX users_username_lower_unique
ON users (LOWER(username));

CREATE UNIQUE INDEX users_email_lower_unique
ON users (LOWER(email));


-- 2. CONVERSATIONS

CREATE TABLE conversations (
id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

type VARCHAR(20) NOT NULL,
title VARCHAR(100),

created_by UUID NOT NULL
REFERENCES users(id) ON DELETE RESTRICT,

created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

CONSTRAINT conversations_type_check
CHECK (type IN ('direct', 'group'))
);


-- 3. CONVERSATION MEMBERS

CREATE TABLE conversation_members (
conversation_id UUID NOT NULL
REFERENCES conversations(id) ON DELETE CASCADE,

user_id UUID NOT NULL
REFERENCES users(id) ON DELETE CASCADE,

role VARCHAR(20) NOT NULL DEFAULT 'member',

joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

PRIMARY KEY (conversation_id, user_id),

CONSTRAINT conversation_members_role_check
CHECK (role IN ('owner', 'admin', 'member'))
);

CREATE INDEX conversation_members_user_idx
ON conversation_members(user_id);


-- 4. MESSAGES

CREATE TABLE messages (
id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

conversation_id UUID NOT NULL
REFERENCES conversations(id) ON DELETE CASCADE,

sender_id UUID NOT NULL
REFERENCES users(id) ON DELETE RESTRICT,

body TEXT NOT NULL,

message_type VARCHAR(20) NOT NULL DEFAULT 'text',

edited_at TIMESTAMPTZ,
deleted_at TIMESTAMPTZ,

created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

CONSTRAINT messages_type_check
CHECK (
message_type IN (
'text',
'image',
'file',
'system'
)
)
);

CREATE INDEX messages_conversation_created_idx
ON messages(conversation_id, created_at DESC);

CREATE INDEX messages_sender_idx
ON messages(sender_id);
