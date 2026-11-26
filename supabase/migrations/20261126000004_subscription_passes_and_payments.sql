-- Web-aligned subscription passes (pass-15 / pass-30), daily + wallet credit buckets,
-- single-active-model hourly passes with automatic metering refund, and manual receipt review queue.

-- 1. Extend public.user_credits for time-windowed passes, wallet bucket, and active hourly pass.
alter table public.user_credits
    alter column balance set default 6;

alter table public.user_credits
    add column if not exists wallet_credit bigint not null default 0 check (wallet_credit >= 0),
    add column if not exists paid_until timestamptz,
    add column if not exists has_paid boolean not null default false,
    add column if not exists last_reset_date date not null default (now() at time zone 'utc')::date,
    add column if not exists active_model text,
    add column if not exists pass_started_at timestamptz,
    add column if not exists pass_expires_at timestamptz,
    add column if not exists pass_daily_spent bigint not null default 0 check (pass_daily_spent >= 0),
    add column if not exists pass_wallet_spent bigint not null default 0 check (pass_wallet_spent >= 0),
    add column if not exists pass_session_id uuid;

-- 2. Extend public.app_settings with tier allowances, pass duration, and hosted checkout config.
alter table public.app_settings
    add column if not exists free_daily_credits integer not null default 6 check (free_daily_credits >= 0),
    add column if not exists paid_daily_credits integer not null default 15 check (paid_daily_credits >= 0),
    add column if not exists pass_duration_minutes integer not null default 60 check (pass_duration_minutes > 0),
    add column if not exists payment_link text not null default 'https://sociabuzz.com/minetallestmc/tribe',
    add column if not exists topup_paused boolean not null default false;

-- Update default model catalog to include per-model hourly credit_cost and paid_only flag.
alter table public.app_settings
    alter column models set default '[
        {"id":"openmi-default","label":"Openmi Default","context_window":128000,"credit_cost":3,"paid_only":false},
        {"id":"openai/gpt-4o-mini","label":"GPT-4o Mini","context_window":128000,"credit_cost":2,"paid_only":false},
        {"id":"openai/gpt-4o","label":"GPT-4o","context_window":128000,"credit_cost":3,"paid_only":false},
        {"id":"anthropic/claude-3.5-sonnet","label":"Claude 3.5 Sonnet","context_window":200000,"credit_cost":3,"paid_only":true}
    ]'::jsonb;

-- Backfill credit_cost & paid_only onto any existing models JSON entries missing those keys.
update public.app_settings
set models = (
    select jsonb_agg(
        case
            when elem ? 'credit_cost' and elem ? 'paid_only' then elem
            else elem
                || jsonb_build_object(
                    'credit_cost', coalesce((elem->>'credit_cost')::int, 3),
                    'paid_only', coalesce((elem->>'paid_only')::boolean, false)
                )
        end
    )
    from jsonb_array_elements(models) as elem
),
updated_at = now()
where id = 1
  and jsonb_typeof(models) = 'array';

-- 3. Payment receipt requests table (manual Sociabuzz proof-of-payment queue).
create table if not exists public.payment_requests (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null references auth.users (id) on delete cascade,
    plan_id text not null check (plan_id in ('pass-15', 'pass-30')),
    amount_myr numeric(10, 2) not null check (amount_myr >= 0),
    reference text not null check (char_length(trim(reference)) between 1 and 120),
    note text not null default '' check (char_length(note) <= 500),
    receipt_path text not null,
    status text not null default 'pending' check (status in ('pending', 'approved', 'rejected')),
    credit_granted bigint not null default 0 check (credit_granted >= 0),
    decided_by text,
    decided_at timestamptz,
    decision_note text not null default '',
    created_at timestamptz not null default now()
);

create index if not exists idx_payment_requests_user_created
    on public.payment_requests (user_id, created_at desc);

create index if not exists idx_payment_requests_status_created
    on public.payment_requests (status, created_at asc);

create unique index if not exists idx_payment_requests_active_reference
    on public.payment_requests (lower(trim(reference)))
    where status <> 'rejected';

alter table public.payment_requests enable row level security;
revoke all on public.payment_requests from public, anon, authenticated;
grant select on public.payment_requests to authenticated;
grant select, insert, update, delete on public.payment_requests to service_role;

drop policy if exists "users_read_own_payment_requests" on public.payment_requests;
create policy "users_read_own_payment_requests"
    on public.payment_requests
    for select
    to authenticated
    using (auth.uid() = user_id);

-- 4. Credit ledger table (audit trail for topups, hourly charges, refunds, and admin grants).
create table if not exists public.credit_ledger (
    id uuid primary key default gen_random_uuid(),
    user_id uuid not null references auth.users (id) on delete cascade,
    kind text not null check (kind in ('topup', 'hour_charge', 'hour_refund', 'admin_grant', 'admin_pass')),
    daily_delta bigint not null default 0,
    wallet_delta bigint not null default 0,
    reference text,
    note text not null default '',
    created_at timestamptz not null default now()
);

create index if not exists idx_credit_ledger_user_created
    on public.credit_ledger (user_id, created_at desc);

alter table public.credit_ledger enable row level security;
revoke all on public.credit_ledger from public, anon, authenticated;
grant select on public.credit_ledger to authenticated;
grant select, insert, update, delete on public.credit_ledger to service_role;

drop policy if exists "users_read_own_credit_ledger" on public.credit_ledger;
create policy "users_read_own_credit_ledger"
    on public.credit_ledger
    for select
    to authenticated
    using (auth.uid() = user_id);

-- 5. Private storage bucket for uploaded payment receipts (5 MB limit, allowlisted MIME types).
do $$
begin
    insert into storage.buckets (id, name, public, file_size_limit, allowed_mime_types)
    values (
        'receipts',
        'receipts',
        false,
        5242880,
        array['image/png', 'image/jpeg', 'image/webp', 'application/pdf']
    )
    on conflict (id) do update set
        public = false,
        file_size_limit = excluded.file_size_limit,
        allowed_mime_types = excluded.allowed_mime_types;
exception
    when undefined_table then
        -- Storage schema may not exist in standalone postgres test instances.
        null;
end;
$$;

-- 6. Helper RPC: sync_user_daily_credits(p_user_id uuid)
-- Ensures row exists, settles 00:00 UTC daily allowance (no rollover), and returns full status.
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
    v_payment_link text := 'https://sociabuzz.com/minetallestmc/tribe';
    v_topup_paused boolean := false;
    v_row public.user_credits%rowtype;
    v_is_paid boolean;
    v_daily_limit integer;
    v_effective_plan text;
    v_active_model text;
    v_pass_expires_at timestamptz;
begin
    select
        coalesce(free_daily_credits, 6),
        coalesce(paid_daily_credits, 15),
        coalesce(nullif(trim(payment_link), ''), 'https://sociabuzz.com/minetallestmc/tribe'),
        coalesce(topup_paused, false)
    into v_free_daily, v_paid_daily, v_payment_link, v_topup_paused
    from public.app_settings
    where id = 1;

    insert into public.user_credits (user_id, plan, balance, wallet_credit, last_reset_date, updated_at)
    values (p_user_id, 'free', v_free_daily, 0, v_today, v_now)
    on conflict (user_id) do nothing;

    select * into v_row
    from public.user_credits
    where user_id = p_user_id
    for update;

    v_is_paid := (v_row.paid_until is not null and v_row.paid_until > v_now);
    v_effective_plan := case when v_is_paid then 'paid' else 'free' end;
    v_daily_limit := case when v_is_paid then v_paid_daily else v_free_daily end;

    if v_row.last_reset_date < v_today then
        v_row.balance := v_daily_limit;
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

-- 7. Atomic RPC: acquire_model_pass
-- Enforces daily reset, paid_only gating, single active hourly pass reuse, and
-- daily-first + wallet-remainder charging (all-or-nothing).
create or replace function public.acquire_model_pass(
    p_user_id uuid,
    p_model_id text,
    p_credit_cost bigint,
    p_paid_only boolean,
    p_duration_minutes integer default 60
)
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
    v_payment_link text := 'https://sociabuzz.com/minetallestmc/tribe';
    v_duration integer := greatest(1, coalesce(p_duration_minutes, 60));
    v_cost bigint := greatest(0, coalesce(p_credit_cost, 3));
    v_row public.user_credits%rowtype;
    v_is_paid boolean;
    v_daily_limit integer;
    v_effective_plan text;
    v_daily_spent bigint := 0;
    v_wallet_spent bigint := 0;
    v_session_id uuid;
    v_expires_at timestamptz;
begin
    select
        coalesce(free_daily_credits, 6),
        coalesce(paid_daily_credits, 15),
        coalesce(nullif(trim(payment_link), ''), 'https://sociabuzz.com/minetallestmc/tribe')
    into v_free_daily, v_paid_daily, v_payment_link
    from public.app_settings
    where id = 1;

    insert into public.user_credits (user_id, plan, balance, wallet_credit, last_reset_date, updated_at)
    values (p_user_id, 'free', v_free_daily, 0, v_today, v_now)
    on conflict (user_id) do nothing;

    select * into v_row
    from public.user_credits
    where user_id = p_user_id
    for update;

    v_is_paid := (v_row.paid_until is not null and v_row.paid_until > v_now);
    v_effective_plan := case when v_is_paid then 'paid' else 'free' end;
    v_daily_limit := case when v_is_paid then v_paid_daily else v_free_daily end;

    if v_row.last_reset_date < v_today then
        v_row.balance := v_daily_limit;
        v_row.last_reset_date := v_today;
    end if;

    -- Gate paid-only models before charging anything.
    if coalesce(p_paid_only, false) and not v_is_paid then
        update public.user_credits
        set plan = v_effective_plan,
            balance = v_row.balance,
            last_reset_date = v_row.last_reset_date,
            updated_at = v_now
        where user_id = p_user_id;

        return jsonb_build_object(
            'status', 'plan_required',
            'plan', v_effective_plan,
            'is_paid', v_is_paid,
            'balance', v_row.balance,
            'wallet_credit', v_row.wallet_credit,
            'daily_limit', v_daily_limit,
            'next_reset_at', v_next_reset,
            'payment_link', v_payment_link
        );
    end if;

    -- Reuse active pass if it matches the requested model and has not expired (server time).
    if v_row.active_model = p_model_id
       and v_row.pass_expires_at is not null
       and v_row.pass_expires_at > v_now then
        update public.user_credits
        set plan = v_effective_plan,
            balance = v_row.balance,
            last_reset_date = v_row.last_reset_date,
            updated_at = v_now
        where user_id = p_user_id;

        return jsonb_build_object(
            'status', 'active',
            'plan', v_effective_plan,
            'is_paid', v_is_paid,
            'balance', v_row.balance,
            'wallet_credit', v_row.wallet_credit,
            'daily_limit', v_daily_limit,
            'next_reset_at', v_next_reset,
            'active_model', v_row.active_model,
            'pass_expires_at', v_row.pass_expires_at,
            'session_id', v_row.pass_session_id,
            'daily_spent', 0,
            'wallet_spent', 0,
            'payment_link', v_payment_link
        );
    end if;

    -- Need to start a new 1-hour pass (replaces any previous active model pass).
    if (v_row.balance + v_row.wallet_credit) < v_cost then
        -- Persist daily reset if it occurred, but preserve any existing active pass untouched.
        update public.user_credits
        set plan = v_effective_plan,
            balance = v_row.balance,
            last_reset_date = v_row.last_reset_date,
            updated_at = v_now
        where user_id = p_user_id;

        return jsonb_build_object(
            'status', 'insufficient_credits',
            'plan', v_effective_plan,
            'is_paid', v_is_paid,
            'balance', v_row.balance,
            'wallet_credit', v_row.wallet_credit,
            'daily_limit', v_daily_limit,
            'required', v_cost,
            'next_reset_at', v_next_reset,
            'active_model', case when v_row.pass_expires_at > v_now then v_row.active_model else null end,
            'pass_expires_at', case when v_row.pass_expires_at > v_now then v_row.pass_expires_at else null end,
            'payment_link', v_payment_link
        );
    end if;

    -- Charge daily balance first, wallet only for the remainder.
    v_daily_spent := least(v_row.balance, v_cost);
    v_wallet_spent := v_cost - v_daily_spent;
    v_row.balance := v_row.balance - v_daily_spent;
    v_row.wallet_credit := v_row.wallet_credit - v_wallet_spent;
    v_session_id := gen_random_uuid();
    v_expires_at := v_now + make_interval(mins => v_duration);

    update public.user_credits
    set plan = v_effective_plan,
        balance = v_row.balance,
        wallet_credit = v_row.wallet_credit,
        last_reset_date = v_row.last_reset_date,
        active_model = p_model_id,
        pass_started_at = v_now,
        pass_expires_at = v_expires_at,
        pass_daily_spent = v_daily_spent,
        pass_wallet_spent = v_wallet_spent,
        pass_session_id = v_session_id,
        updated_at = v_now
    where user_id = p_user_id;

    if v_cost > 0 then
        insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
        values (
            p_user_id,
            'hour_charge',
            -v_daily_spent,
            -v_wallet_spent,
            'session:' || v_session_id::text,
            '1-hour pass for ' || p_model_id
        );
    end if;

    return jsonb_build_object(
        'status', 'purchased',
        'plan', v_effective_plan,
        'is_paid', v_is_paid,
        'balance', v_row.balance,
        'wallet_credit', v_row.wallet_credit,
        'daily_limit', v_daily_limit,
        'next_reset_at', v_next_reset,
        'active_model', p_model_id,
        'pass_expires_at', v_expires_at,
        'session_id', v_session_id,
        'daily_spent', v_daily_spent,
        'wallet_spent', v_wallet_spent,
        'payment_link', v_payment_link
    );
end;
$$;

revoke all on function public.acquire_model_pass(uuid, text, bigint, boolean, integer) from public;
grant execute on function public.acquire_model_pass(uuid, text, bigint, boolean, integer) to service_role;

-- 8. Automatic metering refund RPC: refund_hour_session
-- Within 1 hour, once per session, returns exact daily_spent and wallet_spent to their
-- original buckets and ends the hour so the user does not keep both Credit and time.
create or replace function public.refund_hour_session(p_user_id uuid, p_session_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
    v_now timestamptz := now();
    v_row public.user_credits%rowtype;
    v_daily_refund bigint;
    v_wallet_refund bigint;
begin
    if p_session_id is null then
        return jsonb_build_object('refunded', false);
    end if;

    select * into v_row
    from public.user_credits
    where user_id = p_user_id
    for update;

    if not found
       or v_row.pass_session_id is distinct from p_session_id
       or v_row.pass_started_at is null
       or v_row.pass_started_at < v_now - interval '1 hour' then
        return jsonb_build_object('refunded', false);
    end if;

    v_daily_refund := coalesce(v_row.pass_daily_spent, 0);
    v_wallet_refund := coalesce(v_row.pass_wallet_spent, 0);

    update public.user_credits
    set balance = balance + v_daily_refund,
        wallet_credit = wallet_credit + v_wallet_refund,
        active_model = null,
        pass_started_at = null,
        pass_expires_at = null,
        pass_daily_spent = 0,
        pass_wallet_spent = 0,
        pass_session_id = null,
        updated_at = v_now
    where user_id = p_user_id
    returning * into v_row;

    if (v_daily_refund + v_wallet_refund) > 0 then
        insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
        values (
            p_user_id,
            'hour_refund',
            v_daily_refund,
            v_wallet_refund,
            'session:' || p_session_id::text,
            'Automatic refund on upstream error'
        );
    end if;

    return jsonb_build_object(
        'refunded', true,
        'daily_refunded', v_daily_refund,
        'wallet_refunded', v_wallet_refund,
        'balance', v_row.balance,
        'wallet_credit', v_row.wallet_credit
    );
end;
$$;

revoke all on function public.refund_hour_session(uuid, uuid) from public;
grant execute on function public.refund_hour_session(uuid, uuid) to service_role;

-- 9. Receipt submission RPC: submit_payment_request
-- Mirrors Web protections: plan price floor (RM 18 / RM 36), owner-path prefix,
-- advisory-locked single-use reference, max 5 pending per account.
create or replace function public.submit_payment_request(
    p_user_id uuid,
    p_plan_id text,
    p_amount_myr numeric,
    p_reference text,
    p_note text,
    p_receipt_path text
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
    v_ref text := trim(coalesce(p_reference, ''));
    v_note text := left(trim(coalesce(p_note, '')), 500);
    v_path text := trim(coalesce(p_receipt_path, ''));
    v_min_amount numeric(10, 2);
    v_pending_count integer;
    v_existing_id uuid;
    v_inserted public.payment_requests%rowtype;
begin
    if p_user_id is null then
        raise exception 'Authentication required';
    end if;

    if p_plan_id = 'pass-15' then
        v_min_amount := 18.00;
    elsif p_plan_id = 'pass-30' then
        v_min_amount := 36.00;
    else
        raise exception 'Invalid plan. Choose pass-15 or pass-30.';
    end if;

    if char_length(v_ref) < 1 or char_length(v_ref) > 120 then
        raise exception 'Payment reference is required (1-120 characters).';
    end if;

    if p_amount_myr is null or p_amount_myr < v_min_amount or p_amount_myr > 100000 then
        raise exception 'Amount must be at least RM % for %.', v_min_amount, p_plan_id;
    end if;

    if position(p_user_id::text || '/' in v_path) <> 1 then
        raise exception 'Invalid receipt path.';
    end if;

    -- Serialize concurrent submissions for the same reference string.
    perform pg_advisory_xact_lock(hashtext(lower(v_ref)));

    select id into v_existing_id
    from public.payment_requests
    where lower(trim(reference)) = lower(v_ref)
      and status <> 'rejected'
    limit 1;

    if v_existing_id is not null then
        raise exception 'This payment reference has already been submitted.';
    end if;

    select count(*) into v_pending_count
    from public.payment_requests
    where user_id = p_user_id
      and status = 'pending';

    if v_pending_count >= 5 then
        raise exception 'You already have 5 pending receipts awaiting review.';
    end if;

    insert into public.payment_requests (
        user_id, plan_id, amount_myr, reference, note, receipt_path
    )
    values (
        p_user_id, p_plan_id, round(p_amount_myr, 2), v_ref, v_note, v_path
    )
    returning * into v_inserted;

    return to_jsonb(v_inserted);
end;
$$;

revoke all on function public.submit_payment_request(uuid, text, numeric, text, text, text) from public;
grant execute on function public.submit_payment_request(uuid, text, numeric, text, text, text) to service_role;

-- 10. Operator review RPC: review_payment_request
-- Row-locked FOR UPDATE, refuses non-pending, stacks paid_until, adds upfront wallet_credit,
-- performs same-day top-up to paid_daily_credits, and logs ledger entry.
create or replace function public.review_payment_request(
    p_id uuid,
    p_decision text,
    p_note text default '',
    p_credit bigint default null,
    p_decided_by text default 'admin'
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
    v_now timestamptz := now();
    v_today date := (v_now at time zone 'utc')::date;
    v_req public.payment_requests%rowtype;
    v_days integer;
    v_default_credit bigint;
    v_credit bigint;
    v_paid_daily integer := 15;
    v_free_daily integer := 6;
    v_user public.user_credits%rowtype;
    v_new_paid_until timestamptz;
    v_new_balance bigint;
begin
    if p_decision not in ('approved', 'rejected') then
        raise exception 'Decision must be approved or rejected.';
    end if;

    select * into v_req
    from public.payment_requests
    where id = p_id
    for update;

    if not found then
        raise exception 'Payment request not found.';
    end if;

    if v_req.status <> 'pending' then
        raise exception 'Payment request has already been %.', v_req.status;
    end if;

    if v_req.plan_id = 'pass-15' then
        v_days := 15;
        v_default_credit := 2;
    elsif v_req.plan_id = 'pass-30' then
        v_days := 30;
        v_default_credit := 3;
    else
        raise exception 'Unsupported plan_id: %', v_req.plan_id;
    end if;

    v_credit := case
        when p_decision = 'approved' then least(100000, greatest(0, coalesce(p_credit, v_default_credit)))
        else 0
    end;

    if p_decision = 'approved' then
        select coalesce(free_daily_credits, 6), coalesce(paid_daily_credits, 15)
        into v_free_daily, v_paid_daily
        from public.app_settings
        where id = 1;

        insert into public.user_credits (user_id, plan, balance, wallet_credit, last_reset_date, updated_at)
        values (v_req.user_id, 'free', v_free_daily, 0, v_today, v_now)
        on conflict (user_id) do nothing;

        select * into v_user
        from public.user_credits
        where user_id = v_req.user_id
        for update;

        v_new_paid_until := greatest(coalesce(v_user.paid_until, v_now), v_now) + make_interval(days => v_days);
        -- Same-day top-up: immediately raise daily balance to paid_daily_credits if below it.
        v_new_balance := greatest(v_user.balance, v_paid_daily);

        update public.user_credits
        set plan = 'paid',
            paid_until = v_new_paid_until,
            has_paid = true,
            wallet_credit = wallet_credit + v_credit,
            balance = v_new_balance,
            last_reset_date = v_today,
            updated_at = v_now
        where user_id = v_req.user_id;

        insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
        values (
            v_req.user_id,
            'topup',
            v_new_balance - v_user.balance,
            v_credit,
            'receipt:' || v_req.id::text,
            format('Approved %s (+%s days, +%s wallet credit)', v_req.plan_id, v_days, v_credit)
        );
    end if;

    update public.payment_requests
    set status = p_decision,
        credit_granted = v_credit,
        decided_by = coalesce(nullif(trim(p_decided_by), ''), 'admin'),
        decided_at = v_now,
        decision_note = left(trim(coalesce(p_note, '')), 500)
    where id = p_id
    returning * into v_req;

    return jsonb_build_object(
        'request', to_jsonb(v_req),
        'paid_until', v_new_paid_until,
        'credit_granted', v_credit
    );
end;
$$;

revoke all on function public.review_payment_request(uuid, text, text, bigint, text) from public;
grant execute on function public.review_payment_request(uuid, text, text, bigint, text) to service_role;

-- 11. Admin manual grant & pass comp RPCs
create or replace function public.admin_grant_credit(
    p_user_id uuid,
    p_wallet_delta bigint,
    p_note text default '',
    p_actor text default 'admin'
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
    v_now timestamptz := now();
    v_today date := (v_now at time zone 'utc')::date;
    v_free_daily integer := 6;
    v_row public.user_credits%rowtype;
    v_new_wallet bigint;
begin
    select coalesce(free_daily_credits, 6) into v_free_daily
    from public.app_settings where id = 1;

    insert into public.user_credits (user_id, plan, balance, wallet_credit, last_reset_date, updated_at)
    values (p_user_id, 'free', v_free_daily, 0, v_today, v_now)
    on conflict (user_id) do nothing;

    select * into v_row
    from public.user_credits
    where user_id = p_user_id
    for update;

    v_new_wallet := greatest(0, v_row.wallet_credit + coalesce(p_wallet_delta, 0));

    update public.user_credits
    set wallet_credit = v_new_wallet,
        updated_at = v_now
    where user_id = p_user_id;

    insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
    values (
        p_user_id,
        'admin_grant',
        0,
        v_new_wallet - v_row.wallet_credit,
        'admin:' || coalesce(nullif(trim(p_actor), ''), 'admin'),
        left(trim(coalesce(p_note, 'Manual wallet adjustment')), 500)
    );

    select * into v_row from public.user_credits where user_id = p_user_id;
    return to_jsonb(v_row);
end;
$$;

revoke all on function public.admin_grant_credit(uuid, bigint, text, text) from public;
grant execute on function public.admin_grant_credit(uuid, bigint, text, text) to service_role;

create or replace function public.admin_set_pass(
    p_user_id uuid,
    p_days integer,
    p_note text default '',
    p_actor text default 'admin'
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
    v_now timestamptz := now();
    v_today date := (v_now at time zone 'utc')::date;
    v_free_daily integer := 6;
    v_paid_daily integer := 15;
    v_row public.user_credits%rowtype;
    v_new_paid_until timestamptz;
    v_new_balance bigint;
begin
    select coalesce(free_daily_credits, 6), coalesce(paid_daily_credits, 15)
    into v_free_daily, v_paid_daily
    from public.app_settings where id = 1;

    insert into public.user_credits (user_id, plan, balance, wallet_credit, last_reset_date, updated_at)
    values (p_user_id, 'free', v_free_daily, 0, v_today, v_now)
    on conflict (user_id) do nothing;

    select * into v_row
    from public.user_credits
    where user_id = p_user_id
    for update;

    if coalesce(p_days, 0) > 0 then
        v_new_paid_until := greatest(coalesce(v_row.paid_until, v_now), v_now) + make_interval(days => p_days);
        v_new_balance := greatest(v_row.balance, v_paid_daily);
        update public.user_credits
        set plan = 'paid',
            paid_until = v_new_paid_until,
            has_paid = true,
            balance = v_new_balance,
            last_reset_date = v_today,
            updated_at = v_now
        where user_id = p_user_id
        returning * into v_row;
    else
        update public.user_credits
        set plan = 'free',
            paid_until = null,
            balance = least(v_row.balance, v_free_daily),
            updated_at = v_now
        where user_id = p_user_id
        returning * into v_row;
    end if;

    insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
    values (
        p_user_id,
        'admin_pass',
        0,
        0,
        'admin:' || coalesce(nullif(trim(p_actor), ''), 'admin'),
        left(trim(coalesce(p_note, case when coalesce(p_days, 0) > 0 then format('Granted +%s pass days', p_days) else 'Revoked paid pass' end)), 500)
    );

    return to_jsonb(v_row);
end;
$$;

revoke all on function public.admin_set_pass(uuid, integer, text, text) from public;
grant execute on function public.admin_set_pass(uuid, integer, text, text) to service_role;
