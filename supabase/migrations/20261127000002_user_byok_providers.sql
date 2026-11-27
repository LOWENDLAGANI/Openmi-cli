-- User-owned BYOK (Bring Your Own Key) LLM providers & custom models.
-- Managed exclusively by the authenticated `providers` Edge Function (service_role)
-- so API keys remain AES-256-GCM encrypted at rest and are never readable by clients.

create table if not exists public.user_providers (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  name text not null,
  preset text not null default 'custom',
  base_url text not null,
  api_key_enc text not null,
  api_key_hint text not null,
  models jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_user_providers_user_id
  on public.user_providers (user_id, created_at asc);

alter table public.user_providers enable row level security;

-- No direct anon/authenticated policies: all access is mediated by Edge Functions
-- using service_role after validating the caller's Supabase JWT.
revoke all on table public.user_providers from anon, authenticated;
