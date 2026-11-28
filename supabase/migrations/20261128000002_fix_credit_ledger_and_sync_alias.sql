-- Fix credit_ledger.kind check constraint, add sync_user_credits alias,
-- and lock down windows_waitlist table privileges.

-- 1. Allow 'redeem_code' and 'referral_bonus' in credit_ledger.kind
alter table public.credit_ledger
  drop constraint if exists credit_ledger_kind_check;

alter table public.credit_ledger
  add constraint credit_ledger_kind_check
  check (kind in (
    'topup',
    'hour_charge',
    'hour_refund',
    'admin_grant',
    'admin_pass',
    'redeem_code',
    'referral_bonus'
  ));

-- 2. Alias sync_user_credits -> sync_user_daily_credits so redeem_coin_code,
--    settle_referral_on_paid, and any RPC callers work without error.
create or replace function public.sync_user_credits(p_user_id uuid)
returns jsonb
language sql
security definer
set search_path = public
as $$
  select public.sync_user_daily_credits(p_user_id);
$$;

revoke all on function public.sync_user_credits(uuid) from public;
grant execute on function public.sync_user_credits(uuid) to service_role;

-- 3. Defense-in-depth privilege lockdown on windows_waitlist
revoke all on table public.windows_waitlist from public, anon, authenticated;
grant all on table public.windows_waitlist to service_role;
