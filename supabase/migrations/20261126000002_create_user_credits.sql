-- User plan and credit balance table for `openmi credits` and `chat` billing.

create table if not exists public.user_credits (
    user_id uuid primary key references auth.users (id) on delete cascade,
    plan text not null default 'free',
    balance bigint not null default 100 check (balance >= 0),
    updated_at timestamptz not null default now()
);

alter table public.user_credits enable row level security;

-- Service role (Edge Functions) has full access; authenticated users may read their own row.
grant select, insert, update, delete on public.user_credits to service_role;
grant select on public.user_credits to authenticated;

drop policy if exists "users_read_own_credits" on public.user_credits;
create policy "users_read_own_credits"
    on public.user_credits
    for select
    to authenticated
    using (auth.uid() = user_id);

-- Atomic credit deduction helper used by the `chat` Edge Function.
-- Returns the new balance, or NULL if insufficient credits.
create or replace function public.deduct_user_credits(p_user_id uuid, p_amount bigint)
returns bigint
language plpgsql
security definer
set search_path = public
as $$
declare
    new_balance bigint;
begin
    if p_amount <= 0 then
        select balance into new_balance from public.user_credits where user_id = p_user_id;
        return new_balance;
    end if;

    update public.user_credits
    set balance = balance - p_amount,
        updated_at = now()
    where user_id = p_user_id
      and balance >= p_amount
    returning balance into new_balance;

    return new_balance;
end;
$$;

revoke all on function public.deduct_user_credits(uuid, bigint) from public;
grant execute on function public.deduct_user_credits(uuid, bigint) to service_role;

comment on table public.user_credits is
    'Per-user plan and remaining credit balance for Openmi CLI.';
