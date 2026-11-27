-- Ensure service_role can read/write public.user_providers from Edge Functions
-- (fixes "permission denied for table user_providers").
revoke all on table public.user_providers from public, anon, authenticated;
grant select, insert, update, delete on table public.user_providers to service_role;
