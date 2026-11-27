-- Update subscription pass price floors:
--   pass-15: RM 14.99
--   pass-30: RM 29.00

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
        v_min_amount := 14.99;
    elsif p_plan_id = 'pass-30' then
        v_min_amount := 29.00;
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
