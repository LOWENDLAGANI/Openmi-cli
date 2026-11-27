-- Configurable Upstash Redis rate limits, concurrency caps, payload/output caps, and global circuit breakers.
-- Defaults are tuned so normal Free-tier agentic coding ("implement feature" with rapid tool loops)
-- runs smoothly without hitting limits, while automated abuse, parallel token-cap bypasses, and scraping are blocked.

alter table public.app_settings
    add column if not exists free_rpm integer not null default 30 check (free_rpm >= 0),
    add column if not exists paid_rpm integer not null default 60 check (paid_rpm >= 0),
    add column if not exists free_burst_10s integer not null default 10 check (free_burst_10s >= 0),
    add column if not exists paid_burst_10s integer not null default 20 check (paid_burst_10s >= 0),
    add column if not exists free_tpm bigint not null default 400000 check (free_tpm >= 0),
    add column if not exists paid_tpm bigint not null default 1000000 check (paid_tpm >= 0),
    add column if not exists free_max_concurrency integer not null default 3 check (free_max_concurrency >= 1),
    add column if not exists paid_max_concurrency integer not null default 6 check (paid_max_concurrency >= 1),
    add column if not exists free_max_input_tokens integer not null default 100000 check (free_max_input_tokens >= 0),
    add column if not exists paid_max_input_tokens integer not null default 180000 check (paid_max_input_tokens >= 0),
    add column if not exists free_max_output_tokens integer not null default 8192 check (free_max_output_tokens >= 256),
    add column if not exists paid_max_output_tokens integer not null default 16384 check (paid_max_output_tokens >= 256),
    add column if not exists ip_chat_rpm integer not null default 90 check (ip_chat_rpm >= 0),
    add column if not exists global_chat_rpm integer not null default 600 check (global_chat_rpm >= 0),
    add column if not exists global_daily_token_limit bigint not null default 150000000 check (global_daily_token_limit >= 0);
