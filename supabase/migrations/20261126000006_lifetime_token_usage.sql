-- Track cumulative lifetime AI token usage (input + output) from account creation.

alter table public.user_credits
    add column if not exists total_tokens_used bigint not null default 0 check (total_tokens_used >= 0);

-- Backfill any existing daily usage into lifetime total.
update public.user_credits
set total_tokens_used = greatest(total_tokens_used, daily_tokens_used)
where daily_tokens_used > total_tokens_used;

-- Update sync_user_daily_credits to include total_tokens_used in its response.
create or replace function public.sync_user_daily_credits(p_user_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
    v_now timestamptz := now();
    v_today date := (v_now at time zone 'utc')::date;
    v_next_reset timestamptz := ((v_today + 1)::timestamp at time zone 'utc');
    v_free_daily integer := 6;
    v_paid_daily integer := 15;
    v_free_daily_tokens bigint := 2000000;
    v_paid_daily_tokens bigint := 7000000;
    v_free_max_steps integer := 40;
    v_paid_max_steps integer := 70;
    v_payment_link text := 'https://sociabuzz.com/minetallestmc/tribe';
    v_topup_paused boolean := false;
    v_row public.user_credits%rowtype;
    v_is_paid boolean;
    v_daily_limit integer;
    v_daily_token_limit bigint;
    v_max_steps integer;
    v_effective_plan text;
    v_active_model text;
    v_pass_expires_at timestamptz;
begin
    select
        coalesce(free_daily_credits, 6),
        coalesce(paid_daily_credits, 15),
        coalesce(free_daily_token_limit, 2000000),
        coalesce(paid_daily_token_limit, 7000000),
        coalesce(free_max_steps, 40),
        coalesce(paid_max_steps, 70),
        coalesce(nullif(trim(payment_link), ''), 'https://sociabuzz.com/minetallestmc/tribe'),
        coalesce(topup_paused, false)
    into
        v_free_daily,
        v_paid_daily,
        v_free_daily_tokens,
        v_paid_daily_tokens,
        v_free_max_steps,
        v_paid_max_steps,
        v_payment_link,
        v_topup_paused
    from public.app_settings
    where id = 1;

    insert into public.user_credits (user_id, plan, balance, wallet_credit, daily_tokens_used, total_tokens_used, last_reset_date, updated_at)
    values (p_user_id, 'free', v_free_daily, 0, 0, 0, v_today, v_now)
    on conflict (user_id) do nothing;

    select * into v_row
    from public.user_credits
    where user_id = p_user_id
    for update;

    v_is_paid := (v_row.paid_until is not null and v_row.paid_until > v_now);
    v_effective_plan := case when v_is_paid then 'paid' else 'free' end;
    v_daily_limit := case when v_is_paid then v_paid_daily else v_free_daily end;
    v_daily_token_limit := case when v_is_paid then v_paid_daily_tokens else v_free_daily_tokens end;
    v_max_steps := case when v_is_paid then v_paid_max_steps else v_free_max_steps end;

    if v_row.last_reset_date < v_today then
        v_row.balance := v_daily_limit;
        v_row.daily_tokens_used := 0;
        v_row.last_reset_date := v_today;
    end if;

    if v_row.pass_expires_at is not null and v_row.pass_expires_at <= v_now then
        v_row.active_model := null;
        v_row.pass_started_at := null;
        v_row.pass_expires_at := null;
        v_row.pass_daily_spent := 0;
        v_row.pass_wallet_spent := 0;
        v_row.pass_session_id := null;
    end if;

    update public.user_credits
    set plan = v_effective_plan,
        balance = v_row.balance,
        daily_tokens_used = v_row.daily_tokens_used,
        last_reset_date = v_row.last_reset_date,
        active_model = v_row.active_model,
        pass_started_at = v_row.pass_started_at,
        pass_expires_at = v_row.pass_expires_at,
        pass_daily_spent = v_row.pass_daily_spent,
        pass_wallet_spent = v_row.pass_wallet_spent,
        pass_session_id = v_row.pass_session_id,
        updated_at = v_now
    where user_id = p_user_id;

    v_active_model := v_row.active_model;
    v_pass_expires_at := v_row.pass_expires_at;

    return jsonb_build_object(
        'plan', v_effective_plan,
        'is_paid', v_is_paid,
        'has_paid', v_row.has_paid,
        'paid_until', v_row.paid_until,
        'balance', v_row.balance,
        'wallet_credit', v_row.wallet_credit,
        'daily_limit', v_daily_limit,
        'daily_tokens_used', v_row.daily_tokens_used,
        'daily_token_limit', v_daily_token_limit,
        'total_tokens_used', v_row.total_tokens_used,
        'max_steps', v_max_steps,
        'next_reset_at', v_next_reset,
        'active_model', v_active_model,
        'pass_expires_at', v_pass_expires_at,
        'payment_link', v_payment_link,
        'topup_paused', v_topup_paused
    );
end;
$$;

revoke all on function public.sync_user_daily_credits(uuid) from public;
grant execute on function public.sync_user_daily_credits(uuid) to service_role;

-- Update record_token_usage to accumulate both daily_tokens_used and lifetime total_tokens_used.
create or replace function public.record_token_usage(p_user_id uuid, p_tokens bigint)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
    v_now timestamptz := now();
    v_today date := (v_now at time zone 'utc')::date;
    v_delta bigint := greatest(0, coalesce(p_tokens, 0));
    v_free_daily integer := 6;
    v_paid_daily integer := 15;
    v_free_daily_tokens bigint := 2000000;
    v_paid_daily_tokens bigint := 7000000;
    v_row public.user_credits%rowtype;
    v_is_paid boolean;
    v_daily_limit integer;
    v_daily_token_limit bigint;
begin
    select
        coalesce(free_daily_credits, 6),
        coalesce(paid_daily_credits, 15),
        coalesce(free_daily_token_limit, 2000000),
        coalesce(paid_daily_token_limit, 7000000)
    into v_free_daily, v_paid_daily, v_free_daily_tokens, v_paid_daily_tokens
    from public.app_settings
    where id = 1;

    insert into public.user_credits (user_id, plan, balance, wallet_credit, daily_tokens_used, total_tokens_used, last_reset_date, updated_at)
    values (p_user_id, 'free', v_free_daily, 0, 0, 0, v_today, v_now)
    on conflict (user_id) do nothing;

    select * into v_row
    from public.user_credits
    where user_id = p_user_id
    for update;

    v_is_paid := (v_row.paid_until is not null and v_row.paid_until > v_now);
    v_daily_limit := case when v_is_paid then v_paid_daily else v_free_daily end;
    v_daily_token_limit := case when v_is_paid then v_paid_daily_tokens else v_free_daily_tokens end;

    if v_row.last_reset_date < v_today then
        v_row.balance := v_daily_limit;
        v_row.daily_tokens_used := 0;
        v_row.last_reset_date := v_today;
    end if;

    v_row.daily_tokens_used := v_row.daily_tokens_used + v_delta;
    v_row.total_tokens_used := v_row.total_tokens_used + v_delta;

    update public.user_credits
    set balance = v_row.balance,
        daily_tokens_used = v_row.daily_tokens_used,
        total_tokens_used = v_row.total_tokens_used,
        last_reset_date = v_row.last_reset_date,
        updated_at = v_now
    where user_id = p_user_id;

    return jsonb_build_object(
        'daily_tokens_used', v_row.daily_tokens_used,
        'daily_token_limit', v_daily_token_limit,
        'total_tokens_used', v_row.total_tokens_used
    );
end;
$$;

revoke all on function public.record_token_usage(uuid, bigint) from public;
grant execute on function public.record_token_usage(uuid, bigint) to service_role;
