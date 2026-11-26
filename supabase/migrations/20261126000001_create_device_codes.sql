-- M1: Device-code pairing table for `openmi login`
-- TTL: 10 minutes, single-use, rate-limited polling.

create table if not exists public.device_codes (
    device_code_hash text primary key,
    user_code text not null unique,
    status text not null default 'pending'
        check (status in ('pending', 'approved', 'denied', 'consumed', 'expired')),
    user_id uuid references auth.users (id) on delete cascade,
    user_email text,
    access_token text,
    refresh_token text,
    token_expires_at timestamptz,
    poll_count integer not null default 0,
    last_polled_at timestamptz,
    created_at timestamptz not null default now(),
    expires_at timestamptz not null default (now() + interval '10 minutes')
);

create index if not exists idx_device_codes_user_code
    on public.device_codes (user_code);

create index if not exists idx_device_codes_expires_at
    on public.device_codes (expires_at);

-- Lock down direct client access; only the cli-auth Edge Function (service role) accesses this table.
alter table public.device_codes enable row level security;

comment on table public.device_codes is
    'Short-lived (10m) single-use device pairing codes for Openmi CLI login.';
