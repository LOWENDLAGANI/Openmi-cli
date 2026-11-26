-- Singleton runtime configuration table controlled by the Openmi Admin Panel.
-- Read by `chat`, `models`, and `credits` Edge Functions; overrides env var defaults when set.

create table if not exists public.app_settings (
    id smallint primary key default 1 check (id = 1),
    ai_enabled boolean not null default true,
    llm_base_url text not null default 'https://openrouter.ai/api/v1',
    llm_api_key text not null default '',
    llm_default_model text not null default 'openai/gpt-4o-mini',
    credit_cost integer not null default 1 check (credit_cost >= 0),
    starter_credits integer not null default 100 check (starter_credits >= 0),
    models jsonb not null default '[
        {"id":"openmi-default","label":"Openmi Default","context_window":128000},
        {"id":"openai/gpt-4o-mini","label":"GPT-4o Mini","context_window":128000},
        {"id":"openai/gpt-4o","label":"GPT-4o","context_window":128000},
        {"id":"anthropic/claude-3.5-sonnet","label":"Claude 3.5 Sonnet","context_window":200000}
    ]'::jsonb,
    updated_at timestamptz not null default now()
);

-- Seed singleton row if absent.
insert into public.app_settings (id) values (1) on conflict (id) do nothing;

-- Strictly lock down to service_role only (holds llm_api_key).
alter table public.app_settings enable row level security;
revoke all on public.app_settings from public, anon, authenticated;
grant select, insert, update, delete on public.app_settings to service_role;

comment on table public.app_settings is
    'Singleton runtime AI/model/credit configuration managed via the Openmi Admin Panel.';
