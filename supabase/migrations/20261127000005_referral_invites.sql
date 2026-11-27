-- Referral / Invitation Commission System
-- Each user gets a personal invite code/link.
-- Anti-abuse: Coins are ONLY granted after the invited user activates a Paid pass
-- at least once (either via approved Sociabuzz receipt or admin-granted Paid pass).

-- 1. Configurable referral settings in public.app_settings
alter table public.app_settings
  add column if not exists referral_enabled boolean not null default true,
  add column if not exists referral_inviter_coins integer not null default 3 check (referral_inviter_coins >= 0),
  add column if not exists referral_invitee_coins integer not null default 3 check (referral_invitee_coins >= 0);

-- 2. Personal invite codes (1 per user)
create table if not exists public.user_referral_codes (
  user_id uuid primary key references auth.users(id) on delete cascade,
  code text not null unique,
  created_at timestamptz not null default now()
);

-- 3. Referral relationships & payout status
create table if not exists public.referrals (
  id uuid primary key default gen_random_uuid(),
  inviter_id uuid not null references auth.users(id) on delete cascade,
  invitee_id uuid not null unique references auth.users(id) on delete cascade,
  code text not null,
  status text not null default 'pending' check (status in ('pending', 'rewarded')),
  inviter_coins_granted integer not null default 0,
  invitee_coins_granted integer not null default 0,
  created_at timestamptz not null default now(),
  rewarded_at timestamptz,
  constraint referrals_no_self check (inviter_id <> invitee_id)
);

create index if not exists idx_referrals_inviter_id
  on public.referrals (inviter_id, created_at desc);

alter table public.user_referral_codes enable row level security;
alter table public.referrals enable row level security;

revoke all on table public.user_referral_codes from public, anon, authenticated;
grant select, insert, update, delete on table public.user_referral_codes to service_role;

revoke all on table public.referrals from public, anon, authenticated;
grant select, insert, update, delete on table public.referrals to service_role;

-- 4. Get or create a user's personal invite code
create or replace function public.get_or_create_referral_code(p_user_id uuid)
returns text
language plpgsql
security definer
set search_path = public
as $$
declare
  v_code text;
  v_candidate text;
  v_tries integer := 0;
begin
  select code into v_code
  from public.user_referral_codes
  where user_id = p_user_id;

  if found then
    return v_code;
  end if;

  loop
    v_tries := v_tries + 1;
    v_candidate := 'INV-' || upper(substr(replace(gen_random_uuid()::text, '-', ''), 1, 8));
    begin
      insert into public.user_referral_codes (user_id, code)
      values (p_user_id, v_candidate)
      on conflict (user_id) do nothing;

      select code into v_code
      from public.user_referral_codes
      where user_id = p_user_id;

      if found then
        return v_code;
      end if;
    exception when unique_violation then
      if v_tries >= 10 then
        raise;
      end if;
    end;
  end loop;
end;
$$;

-- 5. Apply an invite code for an invited user (pending until their first Paid pass)
create or replace function public.apply_referral_code(
  p_invitee_id uuid,
  p_code text
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  v_norm text := upper(trim(coalesce(p_code, '')));
  v_enabled boolean := true;
  v_inviter_id uuid;
  v_already_paid boolean := false;
  v_existing public.referrals%rowtype;
begin
  if v_norm = '' then
    return jsonb_build_object('status', 'error', 'error', 'invalid_code', 'message', 'Please enter an invite code.');
  end if;

  select coalesce(referral_enabled, true) into v_enabled
  from public.app_settings
  where id = 1;

  if not v_enabled then
    return jsonb_build_object('status', 'error', 'error', 'disabled', 'message', 'The referral program is currently paused.');
  end if;

  select user_id into v_inviter_id
  from public.user_referral_codes
  where upper(code) = v_norm;

  if not found then
    return jsonb_build_object('status', 'error', 'error', 'not_found', 'message', 'Invite code not found.');
  end if;

  if v_inviter_id = p_invitee_id then
    return jsonb_build_object('status', 'error', 'error', 'self_referral', 'message', 'You cannot use your own invite code.');
  end if;

  select * into v_existing
  from public.referrals
  where invitee_id = p_invitee_id;

  if found then
    return jsonb_build_object('status', 'error', 'error', 'already_referred', 'message', 'You have already applied an invite code.');
  end if;

  -- Prevent retroactively applying an invite code after already becoming a Paid user
  select (paid_until is not null) into v_already_paid
  from public.user_credits
  where user_id = p_invitee_id;

  if coalesce(v_already_paid, false) then
    return jsonb_build_object('status', 'error', 'error', 'already_paid', 'message', 'Invite codes can only be applied before your first Paid pass.');
  end if;

  insert into public.referrals (inviter_id, invitee_id, code, status)
  values (v_inviter_id, p_invitee_id, v_norm, 'pending');

  return jsonb_build_object(
    'status', 'ok',
    'code', v_norm,
    'message', 'Invite code linked! Bonus Coins will be granted automatically when you activate your first Paid pass.'
  );
end;
$$;

-- 6. Settle a pending referral when the invited user activates a Paid pass
create or replace function public.settle_referral_on_paid(
  p_invitee_id uuid,
  p_trigger text default 'paid_pass'
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  v_ref public.referrals%rowtype;
  v_enabled boolean := true;
  v_inviter_coins integer := 3;
  v_invitee_coins integer := 3;
begin
  select * into v_ref
  from public.referrals
  where invitee_id = p_invitee_id and status = 'pending'
  for update;

  if not found then
    return jsonb_build_object('settled', false);
  end if;

  select
    coalesce(referral_enabled, true),
    coalesce(referral_inviter_coins, 3),
    coalesce(referral_invitee_coins, 3)
  into v_enabled, v_inviter_coins, v_invitee_coins
  from public.app_settings
  where id = 1;

  if not v_enabled then
    return jsonb_build_object('settled', false, 'reason', 'disabled');
  end if;

  -- Ensure inviter has a user_credits row
  perform public.sync_user_credits(v_ref.inviter_id);

  if v_inviter_coins > 0 then
    update public.user_credits
    set wallet_credit = wallet_credit + v_inviter_coins,
        updated_at = now()
    where user_id = v_ref.inviter_id;

    insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
    values (
      v_ref.inviter_id,
      'referral_bonus',
      0,
      v_inviter_coins,
      v_ref.code,
      'Invite commission: your invited friend upgraded to Paid (+' || v_inviter_coins || ' Coins)'
    );
  end if;

  if v_invitee_coins > 0 then
    update public.user_credits
    set wallet_credit = wallet_credit + v_invitee_coins,
        updated_at = now()
    where user_id = v_ref.invitee_id;

    insert into public.credit_ledger (user_id, kind, daily_delta, wallet_delta, reference, note)
    values (
      v_ref.invitee_id,
      'referral_bonus',
      0,
      v_invitee_coins,
      v_ref.code,
      'Invite welcome bonus: upgraded to Paid via ' || v_ref.code || ' (+' || v_invitee_coins || ' Coins)'
    );
  end if;

  update public.referrals
  set status = 'rewarded',
      inviter_coins_granted = v_inviter_coins,
      invitee_coins_granted = v_invitee_coins,
      rewarded_at = now()
  where id = v_ref.id;

  return jsonb_build_object(
    'settled', true,
    'inviter_coins', v_inviter_coins,
    'invitee_coins', v_invitee_coins,
    'trigger', p_trigger
  );
end;
$$;

-- 7. Trigger on user_credits so ANY Paid pass activation (Sociabuzz approval or admin grant) settles pending referrals
create or replace function public.trg_user_credits_settle_referral()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
begin
  if NEW.plan = 'paid' and NEW.paid_until is not null and NEW.paid_until > now() then
    perform public.settle_referral_on_paid(NEW.user_id, 'user_credits_paid');
  end if;
  return NEW;
end;
$$;

drop trigger if exists trg_settle_referral_on_paid on public.user_credits;
create trigger trg_settle_referral_on_paid
  after insert or update of plan, paid_until on public.user_credits
  for each row
  execute function public.trg_user_credits_settle_referral();

revoke all on function public.get_or_create_referral_code(uuid) from public, anon, authenticated;
grant execute on function public.get_or_create_referral_code(uuid) to service_role;

revoke all on function public.apply_referral_code(uuid, text) from public, anon, authenticated;
grant execute on function public.apply_referral_code(uuid, text) to service_role;

revoke all on function public.settle_referral_on_paid(uuid, text) from public, anon, authenticated;
grant execute on function public.settle_referral_on_paid(uuid, text) to service_role;
