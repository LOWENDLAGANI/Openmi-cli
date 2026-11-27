# Deploying the Openmi Supabase Backend

Project ref: `umjlmqeghfrfpduhkits` (`https://umjlmqeghfrfpduhkits.supabase.co`).

## 1. Apply database migrations

Run all migrations in order (via `supabase db push` or the SQL editor / Management API):

1. `20261126000001_create_device_codes.sql` — device pairing table for `openmi login`
2. `20261126000002_create_user_credits.sql` — per-user plan, credit balance, and atomic `deduct_user_credits` RPC
3. `20261126000003_create_app_settings.sql` — singleton `app_settings` row (AI on/off, LLM base URL/key/default model, credit cost, model list) edited from the admin panel
4. `20261126000004_subscription_passes_and_payments.sql` — daily + wallet credit buckets, time-limited passes, hourly model passes, and the manual payment receipt review queue

## 2. Configure Edge Function secrets

> [!IMPORTANT]
> `chat` reads `llm_api_key`, `llm_base_url` and `llm_default_model` from `app_settings` **first** and only falls back to `LLM_*` env vars when those columns are empty. Because migration 3 seeds `llm_base_url` and `llm_default_model` with non-empty defaults, change them in the admin panel (or clear them) rather than relying on the env vars. `OPENMI_PAY_URL`, by contrast, overrides `app_settings.payment_link`.

Set secrets in Supabase Dashboard → Edge Functions → Secrets (or `supabase secrets set`):

| Secret | Used by | Default | Purpose |
| --- | --- | --- | --- |
| `LLM_API_KEY` | `chat` | — | API key for your OpenAI-compatible provider (required unless `app_settings.llm_api_key` is set) |
| `LLM_BASE_URL` | `chat` | `https://openrouter.ai/api/v1` | OpenAI-compatible `/v1` base URL |
| `LLM_DEFAULT_MODEL` | `chat` | `openai/gpt-4o-mini` | Upstream model used for `openmi-default` |
| `OPENMI_MODELS_JSON` | `models` | built-in list | JSON array of models served by `models` |
| `OPENMI_PAY_URL` | `chat`, `credits` | `app_settings.payment_link` | Top-up link shown when credits run out |
| `OPENMI_PROVIDER_URL` | `credits` | `<console-origin>/provider` | Web console BYOK `/provider` URL opened from `/model` |
| `BYOK_ENCRYPTION_KEY` | `providers`, `chat` | — | High-entropy secret used to AES-256-GCM encrypt user BYOK API keys at rest (required for BYOK) |
| `OPENMI_CLI_VERIFY_URL` | `cli-auth` | `<origin>/functions/v1/cli-auth/verify` | Custom web approval URL shown during `openmi login` |
| `PAYMENT_LINK` | `payments` | built-in link | Payment page link |
| `TOPUP_PAUSED` | `payments` | off | Pause new top-up submissions |
| `RESEND_API_KEY` | `payments` | — | Send receipt emails via Resend (optional) |
| `RECEIPT_FROM_EMAIL` | `payments` | `Openmi Billing <billing@openmi.local>` | Sender address for receipt emails |
| `RECEIPT_NOTIFY_EMAIL` | `payments` | — | Admin address notified of new receipts |

Credit costs and daily allowances are configured in `app_settings` (`credit_cost`, per-model `credit_cost` in `models`, `free_daily_credits`, `paid_daily_credits`, `pass_duration_minutes`), not via env vars. New users start with the `user_credits.balance` column default (6 after migration 4).

`SUPABASE_URL` and `SUPABASE_SERVICE_ROLE_KEY` are injected automatically by Supabase.

## 3. Deploy Edge Functions

Deploy all six functions with `verify_jwt = false` (matching `supabase/config.toml`; protected endpoints validate the user's bearer token via `admin.auth.getUser`):

```sh
supabase functions deploy cli-auth  --no-verify-jwt
supabase functions deploy models    --no-verify-jwt
supabase functions deploy credits   --no-verify-jwt
supabase functions deploy chat      --no-verify-jwt
supabase functions deploy payments  --no-verify-jwt
supabase functions deploy providers --no-verify-jwt
```

## 4. Smoke test

```sh
# Public model catalog
curl -fsS https://umjlmqeghfrfpduhkits.supabase.co/functions/v1/models

# Credits (expect 401 without token)
curl -i https://umjlmqeghfrfpduhkits.supabase.co/functions/v1/credits

# End-to-end CLI check
openmi models
openmi login
openmi whoami
openmi credits
openmi run "say hello in one sentence" --permission read-only
```
