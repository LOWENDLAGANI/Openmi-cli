-- Coin giveaway & promo redeem codes system.
-- Managed by the Admin Panel (service_role) and redeemed atomically via
-- the authenticated `redeem` Edge Function calling `public.redeem_coin_code`.

create table if not exists public.redeem_codes (
  id uuid primary key default gen_random_uuid(),
  code text not null unique,
  coins integer not null default 0 check (coins >= 0),
  bonus_pass_days integer not null default 0 check (bonus_pass_days >= 0),
  max_uses integer check (max_uses is null or max_uses >= 1),
  uses_count integer not null default 0 check (uses_count >= 0),
  per_user_limit integer not null default 1 check (per_user_limit >= 1),
  eligible_tier text not null default 'all' check (eligible_tier in ('all', 'free', 'paid')),
  expires_at timestamptz,
  is_active boolean not null default true,
  note text not null default '',
  created_by text not null default 'admin-panel',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists idx_redeem_codes_created_at
  on public.redeem_codes (created_at desc);

create table if not exists public.redeem_code_claims (
  id uuid primary key default gen_random_uuid(),
  code_id uuid not null references public.redeem_codes(id) on delete cascade,
  user_id uuid not null references auth.users(id) on delete cascade,
  coins_granted integer not null default 0,
  bonus_pass_days integer not null default 0,
  claimed_at timestamptz not null default now()
);

create index if not exists idx_redeem_code_claims_code_user
  on public.redeem_code_claims (code_id, user_id);

alter table public.redeem_codes enable row level security;
alter table public.redeem_code_claims enable row level security;

revoke all on table public.redeem_codes from public, anon, authenticated;
grant select, insert, update, delete on table public.redeem_codes to service_role;

revoke all on table public.redeem_code_claims from public, anon, authenticated;
grant select, insert, update, delete on table public.redeem_code_claims to service_role;

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
        paid_until = v_new_paid_until,
        updated_at = now()
    where user_id = p_user_id
    returning wallet_credit, paid_until into v_new_wallet, v_new_paid_until;
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

revoke all on function public.redeem_coin_code(uuid, text) from public, anon, authenticated;
grant execute on function public.redeem_coin_code(uuid, text) to service_role;
