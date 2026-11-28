-- Forward migration:
-- 1. Extend admin_set_pass to accept custom p_paid_until timestamptz, own all pass columns
--    on both grant and revoke, and call sync_user_daily_credits.
-- 2. Reconcile has_paid and daily top-up in redeem_coin_code bonus-days branch.

drop function if exists public.admin_set_pass(uuid, integer, text, text);

create or replace function public.admin_set_pass(
    p_user_id uuid,
    p_days integer default 0,
    p_note text default null,
    p_actor text default 'admin',
    p_paid_until timestamptz default null
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
    v_new_balance integer;
    v_is_grant boolean;
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

    v_is_grant := (p_paid_until is not null) or (coalesce(p_days, 0) > 0);

    if v_is_grant then
        if p_paid_until is not null then
            if p_paid_until <= v_now then
                raise exception 'p_paid_until must be in the future';
            end if;
            v_new_paid_until := p_paid_until;
        else
            v_new_paid_until := greatest(coalesce(v_row.paid_until, v_now), v_now) + make_interval(days => p_days);
        end if;
        v_new_balance := greatest(v_row.balance, v_paid_daily);
        update public.user_credits
        set plan = 'paid',
            paid_until = v_new_paid_until,
            has_paid = true,
            balance = v_new_balance,
            last_reset_date = v_today,
            updated_at = v_now
        where user_id = p_user_id;
    else
        update public.user_credits
        set plan = 'free',
            paid_until = null,
            has_paid = false,
            balance = least(v_row.balance, v_free_daily),
            last_reset_date = v_today,
            updated_at = v_now
        where user_id = p_user_id;
    end if;

    perform public.sync_user_daily_credits(p_user_id);

    select * into v_row
    from public.user_credits
    where user_id = p_user_id;

    insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
    values (
        p_user_id,
        'admin_pass',
        0,
        0,
        'admin:' || coalesce(nullif(trim(p_actor), ''), 'admin'),
        left(trim(coalesce(
            p_note,
            case
                when p_paid_until is not null then format('Granted paid pass until %s', to_char(p_paid_until at time zone 'utc', 'YYYY-MM-DD HH24:MI UTC'))
                when coalesce(p_days, 0) > 0 then format('Granted +%s pass days', p_days)
                else 'Revoked paid pass'
            end
        )), 500)
    );

    return to_jsonb(v_row);
end;
$$;

revoke all on function public.admin_set_pass(uuid, integer, text, text, timestamptz) from public;
grant execute on function public.admin_set_pass(uuid, integer, text, text, timestamptz) to service_role;

-- Reconcile has_paid and daily top-up in redeem_coin_code bonus-days branch.
create or replace function public.redeem_coin_code(
  p_user_id uuid,
  p_code text
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  v_norm text := upper(trim(coalesce(p_code, '')));
  v_code public.redeem_codes%rowtype;
  v_user_claims integer := 0;
  v_synced jsonb;
  v_is_paid boolean := false;
  v_paid_daily integer := 15;
  v_today date := (now() at time zone 'utc')::date;
  v_new_wallet integer := 0;
  v_new_paid_until timestamptz := null;
  v_base_until timestamptz;
begin
  if v_norm = '' then
    return jsonb_build_object('status', 'error', 'error', 'invalid_code', 'message', 'Please enter a redeem code.');
  end if;

  select * into v_code
  from public.redeem_codes
  where upper(code) = v_norm
  for update;

  if not found then
    return jsonb_build_object('status', 'error', 'error', 'not_found', 'message', 'Invalid redeem code.');
  end if;

  if not v_code.is_active then
    return jsonb_build_object('status', 'error', 'error', 'inactive', 'message', 'This code is currently paused.');
  end if;

  if v_code.expires_at is not null and v_code.expires_at <= now() then
    return jsonb_build_object('status', 'error', 'error', 'expired', 'message', 'This code has expired.');
  end if;

  if v_code.max_uses is not null and v_code.uses_count >= v_code.max_uses then
    return jsonb_build_object('status', 'error', 'error', 'depleted', 'message', 'This code has reached its maximum number of redemptions.');
  end if;

  select count(*)::integer into v_user_claims
  from public.redeem_code_claims
  where code_id = v_code.id and user_id = p_user_id;

  if v_user_claims >= v_code.per_user_limit then
    return jsonb_build_object('status', 'error', 'error', 'already_claimed', 'message', 'You have already claimed this code.');
  end if;

  -- Ensure user_credits row exists and get current tier
  v_synced := public.sync_user_credits(p_user_id);
  v_is_paid := coalesce((v_synced ->> 'is_paid')::boolean, false);

  if v_code.eligible_tier = 'free' and v_is_paid then
    return jsonb_build_object('status', 'error', 'error', 'tier_ineligible', 'message', 'This code is only eligible for Free tier accounts.');
  end if;

  if v_code.eligible_tier = 'paid' and not v_is_paid then
    return jsonb_build_object('status', 'error', 'error', 'tier_ineligible', 'message', 'This code is only eligible for Paid pass accounts.');
  end if;

  select coalesce(paid_daily_credits, 15)
  into v_paid_daily
  from public.app_settings where id = 1;

  -- Apply Coins & optional bonus Paid Pass days
  if v_code.bonus_pass_days > 0 then
    select case
      when paid_until is not null and paid_until > now() then paid_until
      else now()
    end into v_base_until
    from public.user_credits
    where user_id = p_user_id;

    v_new_paid_until := coalesce(v_base_until, now()) + make_interval(days => v_code.bonus_pass_days);

    update public.user_credits
    set wallet_credit = wallet_credit + v_code.coins,
        plan = 'paid',
        has_paid = true,
        paid_until = v_new_paid_until,
        balance = greatest(balance, v_paid_daily),
        last_reset_date = v_today,
        updated_at = now()
    where user_id = p_user_id
    returning wallet_credit, paid_until into v_new_wallet, v_new_paid_until;

    perform public.sync_user_daily_credits(p_user_id);
  else
    update public.user_credits
    set wallet_credit = wallet_credit + v_code.coins,
        updated_at = now()
    where user_id = p_user_id
    returning wallet_credit, paid_until into v_new_wallet, v_new_paid_until;
  end if;

  update public.redeem_codes
  set uses_count = uses_count + 1,
      updated_at = now()
  where id = v_code.id;

  insert into public.redeem_code_claims (code_id, user_id, coins_granted, bonus_pass_days)
  values (v_code.id, p_user_id, v_code.coins, v_code.bonus_pass_days);

  insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
  values (
    p_user_id,
    'redeem_code',
    0,
    v_code.coins,
    v_code.code,
    case
      when v_code.note <> '' then 'Redeemed ' || v_code.code || ' (' || v_code.note || ')'
      else 'Redeemed code ' || v_code.code
    end
  );

  return jsonb_build_object(
    'status', 'ok',
    'code', v_code.code,
    'coins_granted', v_code.coins,
    'bonus_pass_days', v_code.bonus_pass_days,
    'wallet_credit', v_new_wallet,
    'paid_until', v_new_paid_until
  );
end;
$$;

revoke all on function public.redeem_coin_code(uuid, text) from public;
grant execute on function public.redeem_coin_code(uuid, text) to service_role;
