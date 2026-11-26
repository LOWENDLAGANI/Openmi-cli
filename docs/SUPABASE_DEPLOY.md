# Deploying the Openmi Supabase Backend

Project ref: `umjlmqeghfrfpduhkits` (`https://umjlmqeghfrfpduhkits.supabase.co`).

## 1. Apply database migrations

Run both migrations in order (via `supabase db push` or the SQL editor / Management API):

1. `supabase/migrations/20261126000001_create_device_codes.sql` — device pairing table for `openmi login`
2. `supabase/migrations/20261126000002_create_user_credits.sql` — per-user plan, credit balance, and atomic `deduct_user_credits` RPC

## 2. Configure Edge Function secrets

Set these in Supabase Dashboard → Edge Functions → Secrets (or `supabase secrets set`):

| Secret | Required | Default | Purpose |
| --- | --- | --- | --- |
| `LLM_API_KEY` | **Yes** (for `chat`) | — | API key for your OpenAI-compatible provider (OpenRouter, OpenAI, Groq, DeepSeek, etc.) |
| `LLM_BASE_URL` | No | `https://openrouter.ai/api/v1` | OpenAI-compatible `/v1` base URL |
| `LLM_DEFAULT_MODEL` | No | `openai/gpt-4o-mini` | Upstream model used when CLI requests `openmi-default` |
| `OPENMI_MODELS_JSON` | No | Built-in list | JSON array of `{ id, label, context_window }` served by `models` |
| `OPENMI_CREDIT_COST` | No | `1` | Credits deducted per `chat` step (`0` disables deduction) |
| `OPENMI_STARTER_CREDITS` | No | `100` | Starter credits auto-provisioned for new users |
| `OPENMI_CLI_VERIFY_URL` | No | `<origin>/functions/v1/cli-auth/verify` | Custom web approval URL shown during `openmi login` |

`SUPABASE_URL` and `SUPABASE_SERVICE_ROLE_KEY` are injected automatically by Supabase.

## 3. Deploy Edge Functions

Deploy all four functions with `verify_jwt = false` (each protected endpoint validates the user's Supabase bearer token via `admin.auth.getUser`):

```sh
supabase functions deploy cli-auth --no-verify-jwt
supabase functions deploy models   --no-verify-jwt
supabase functions deploy credits  --no-verify-jwt
supabase functions deploy chat     --no-verify-jwt
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
