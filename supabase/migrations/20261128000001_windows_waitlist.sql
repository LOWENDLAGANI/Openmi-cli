-- Windows App Waitlist signups
create table if not exists public.windows_waitlist (
  id uuid primary key default gen_random_uuid(),
  email text not null,
  email_normalized text generated always as (lower(trim(email))) stored unique,
  status text not null default 'active' check (status in ('active', 'cancelled')),
  source text not null default 'web',
  created_at timestamptz not null default now(),
  cancelled_at timestamptz
);

create index if not exists idx_windows_waitlist_created_at
  on public.windows_waitlist (created_at desc);

create index if not exists idx_windows_waitlist_status
  on public.windows_waitlist (status);

alter table public.windows_waitlist enable row level security;

grant all on table public.windows_waitlist to service_role;
