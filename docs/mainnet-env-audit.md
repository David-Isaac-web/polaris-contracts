# Mainnet env-var audit (#47)

Every env var default across `polaris-contracts`, `polaris-oracle` and
`polaris-frontend`, looking for one specific failure: **a testnet-shaped
default that would silently apply in a mainnet deploy.** This is the
mainnet-blocking item from #15's checklist.

This is a snapshot. Re-run it before mainnet and after any change to a
config file in any of the three repos. The method is below.

| Repo | Commit audited | Files read |
|---|---|---|
| `polaris-contracts` | `ddf598e` | every `contracts/*/src/*.rs`, `Cargo.toml`, `.github/workflows/ci.yml`, `README.md` |
| `polaris-oracle` | `b0f2542` (2026-11-08) | `src/config/configuration.ts`, `.env.example`, `fly.toml`, `Dockerfile`, `.github/workflows/*`, every other `process.env` read in `src/` |
| `polaris-frontend` | `0119e87` (2026-11-10) | `src/lib/config.ts`, `.env.local.example`, `vercel.json`, `next.config.ts`, `README.md`, every `process.env` read in `src/` |

**Legend**
- 🔴 **Blocker.** A testnet-shaped default, or a testnet-only value with no
  mainnet guard. It silently applies if the variable is unset (or copied
  from a testnet env).
- 🟠 **Unsafe production default.** Not tied to a network, but wrong for any
  real deployment. Must be set explicitly.
- 🟢 **Fine.** No default, it fails closed, or the value is the same on
  every network.

## Summary

| Repo | 🔴 | 🟠 | Status |
|---|---|---|---|
| `polaris-contracts` | 0 in code, 1 in docs | 0 | **Clear.** The docs finding is fixed in the same change as this audit. |
| `polaris-oracle` | 9 | 5 | **Blocks mainnet.** Needs a PR in that repo. |
| `polaris-frontend` | 2 | 2 | **Blocks mainnet.** Needs a PR in that repo. |

## `polaris-contracts`

The contracts read no env vars at all. Soroban contracts are `no_std` with
no runtime environment, and there is no `env!`/`option_env!` (compile-time)
read anywhere in the workspace either. CI sets no env vars. Every
network-specific value is an explicit `initialize()` argument with no
default in code: the Lazer verifier, the Reflector and RedStone feeds,
`feed_id`, collateral and treasury. Nothing on-chain can silently carry
over from testnet. A deploy has to pass each value deliberately.

| Item | Finding | |
|---|---|---|
| Env reads (`std::env`, `env!`, `option_env!`) | None | 🟢 |
| `initialize()` params (`feed_id`, `lazer_contract`, `reflector`, `redstone`, `collateral`, `treasury`) | All required at deploy time; none defaulted | 🟢 |
| Test constants (`FEED_ID = 100`, `REFLECTOR_DECIMALS = 14`, …) | `#[cfg(test)]` only, never compiled into wasm | 🟢 |
| `contracts/mock-lazer`, `contracts/mock-redstone` | Separate crates, so deploying one is an explicit choice. Not defaults. No on-chain guard either (see follow-ups). | 🟢 |
| README "Deploying" recipe | Hardcoded `--network testnet`, the testnet Reflector id, the placeholder `--feed_id 100` and `--redstone null` as a single copy-paste command | 🔴 → **fixed**: split into testnet and mainnet recipes, with the mainnet one using explicit placeholders |

## `polaris-oracle`

All defaults live in `src/config/configuration.ts`. The deploy path
(`StellarService`) passes `--network`, `--rpc-url` and
`--network-passphrase` to the Stellar CLI from three *independent*
defaults. So overriding only one of them gives a half-switched config.

| Variable | Default | Effect if unset on mainnet | |
|---|---|---|---|
| `STELLAR_RPC_URL` | `https://soroban-testnet.stellar.org` | Every read, simulation and submission goes to testnet | 🔴 |
| `STELLAR_NETWORK_PASSPHRASE` | `Test SDF Network ; September 2015` | Every tx signed for testnet. `auth-relay.service.ts:83` adds a second `?? Networks.TESTNET` fallback on top. | 🔴 |
| `STELLAR_NETWORK` | `testnet` | Passed as `--network` to every CLI deploy/invoke | 🔴 |
| `XLM_USD_FEED_ID` | `100` | An unverified placeholder Lazer feed id, stored into every market `initialize()` | 🔴 |
| `FEED_CATALOG` | Built-in `[{feedId: 100, …XLM/USD}]` | The market factory creates markets on placeholder feed `100`. A *malformed* value also falls back to this default with only a `console.warn`. | 🔴 |
| `EMAIL_WALLET_ORIGIN` | `http://localhost:3000` | Must equal the origin pinned in the smart-wallet factory's `initialize()` (on-chain since this repo's `e1bb219`). If it doesn't, every email-wallet signature fails with `OriginMismatch`. The `AppConfig` doc comment still says "never validated on-chain", which is stale. `email-wallet-signer.ts:37-41` is correct. | 🔴 |
| `EMAIL_WALLET_RP_ID` | `localhost` | Same pairing as `EMAIL_WALLET_ORIGIN` | 🔴 |
| `FRIENDBOT_URL` + faucet | `https://friendbot.stellar.org`; `POST /markets/faucet` and auto-fund on email login are always on | A testnet-only feature with no off switch. Friendbot doesn't exist on mainnet, so the endpoint stays exposed and fails on every call. | 🔴 |
| `MOCK_REDSTONE_CONTRACT` | none | No default, but nothing refuses it on mainnet. If it's copied from a testnet env, perpetuals get corroborated against a price feed the backend writes itself. | 🔴 (guard) |
| `LAZER_CONTRACT` | none | Same copy-paste risk: `.env.example` suggests `polaris-mock-lazer` on testnet | 🟠 |
| `REFLECTOR_CONTRACT` | none (refuses to deploy if unset) | Fails closed. Only `.env.example` shows the testnet id as an example. | 🟢 |
| `EMAIL_PROVIDER` | `console` | Login codes are written to stdout instead of emailed | 🟠 |
| `EMAIL_WALLET_ENC_KEY`, `SESSION_JWT_SECRET` | Random per boot (with a warning) | Custodial keys become unrecoverable and sessions invalid after every restart | 🟠 |
| `NODE_ENV` | unset (not in `Dockerfile` or `fly.toml`) | Session cookies go out without `Secure` and with `SameSite=Lax`, which breaks cross-site auth. Can't verify from the repo whether it's set as a Fly secret. | 🟠 |
| `CORS_ORIGINS` | empty | Cross-origin requests are rejected. This fails loudly. | 🟠 |
| `ORACLE_SECRET_KEY`, `ADMIN_API_KEY` | `''` | Fail closed: signing fails and admin calls are rejected | 🟢 |
| `PYTH_LAZER_WS_URL`, `PYTH_HERMES_URL` | Pyth production endpoints | Same on every network | 🟢 |
| Numeric tunables (`MARKET_FACTORY_*`, `*_TOLERANCE_BPS`, `*_STALENESS_SECS`, rate limits, TTLs) | Network-neutral values | Not network-shaped. Staleness is re-validated on-chain at `initialize()`. | 🟢 |

**Recommended fix (oracle PR):**
1. Give the network no default. Require an explicit `STELLAR_NETWORK` and
   derive the passphrase from it. Or, at minimum, refuse to boot when the
   three network settings disagree. That catches the half-switched config.
2. When the network is mainnet, refuse to boot unless every 🔴/🟠 variable
   above is set explicitly. Also refuse `MOCK_REDSTONE_CONTRACT`, require
   `EMAIL_PROVIDER=resend`, and disable the faucet routes.
3. Drop the redundant `?? Networks.TESTNET` in `auth-relay.service.ts:83`.
   Fix the stale `emailWalletRpId`/`emailWalletOrigin` doc comment in
   `configuration.ts`.

## `polaris-frontend`

| Variable | Default | Effect if unset on mainnet | |
|---|---|---|---|
| `NEXT_PUBLIC_STELLAR_RPC_URL` | `https://soroban-testnet.stellar.org` | Freighter trades are built and submitted against testnet. **Missing from `README.md`'s production table and from `.env.local.example`**, so following the docs leaves it unset. | 🔴 |
| `NEXT_PUBLIC_STELLAR_NETWORK_PASSPHRASE` | `Test SDF Network ; September 2015` | Same, and also undocumented. The Freighter network check then rejects a correctly configured mainnet wallet with a hardcoded "Switch it to Testnet" (`freighter-wallet.ts:92`). | 🔴 |
| `NEXT_PUBLIC_ORACLE_URL` | `http://localhost:3001` | Every API call fails. This fails loudly. | 🟠 |
| `NEXT_PUBLIC_WEBAUTHN_RP_ID` | n/a | Documented as production-critical, but **no code in `src/` reads it**. Either wire it up or drop it from the docs. | 🟠 (doc drift) |
| `NEXT_PUBLIC_FEATURED_MARKET_ID` | none | Landing-page widget hidden when unset. Must be a mainnet market id if set. | 🟢 |

`NEXT_PUBLIC_*` values are inlined at `next build` time. They have to be in
Vercel's **Production** environment *before* the build, and changing one
later means rebuilding.

**Recommended fix (frontend PR):** document both `NEXT_PUBLIC_STELLAR_*`
variables in the README and `.env.local.example`. Fail a production build
(`VERCEL_ENV === 'production'`) when they're unset. Derive the Freighter
error message and the landing footer's "Stellar testnet" copy from the
configured passphrase.

## Mainnet go/no-go checklist

- [ ] Oracle: the network-default fix and mainnet boot guard above are merged
- [ ] Oracle: `STELLAR_RPC_URL`, `STELLAR_NETWORK_PASSPHRASE` (`Public Global Stellar Network ; September 2015`) and `STELLAR_NETWORK` are set and agree with each other
- [ ] Oracle: `XLM_USD_FEED_ID` and `FEED_CATALOG` use Pyth's real registered Lazer id, not `100`
- [ ] Oracle: `EMAIL_WALLET_ORIGIN`/`EMAIL_WALLET_RP_ID` equal the mainnet smart-wallet factory's pinned origin
- [ ] Oracle: `LAZER_CONTRACT` is the real verifier; `MOCK_REDSTONE_CONTRACT` is unset; faucet is disabled
- [ ] Oracle: `EMAIL_PROVIDER=resend`, `EMAIL_WALLET_ENC_KEY`, `SESSION_JWT_SECRET`, `NODE_ENV=production`, `CORS_ORIGINS` are set
- [ ] Frontend: `NEXT_PUBLIC_STELLAR_RPC_URL`, `NEXT_PUBLIC_STELLAR_NETWORK_PASSPHRASE`, `NEXT_PUBLIC_ORACLE_URL` are set in Vercel Production before the build
- [ ] Contracts: no `mock-lazer`/`mock-redstone` deployed; every `initialize()` uses the mainnet recipe in `README.md` → "Deploying"

## How to re-run this audit

```sh
# contracts: expect no hits outside tests/docs
git grep -n -E 'std::env|option_env!|env!\(' -- 'contracts/*'
# oracle / frontend: every default lives at a `process.env.X ?? <default>`
git grep -n 'process\.env' -- 'src/*' ':!*.spec.ts' ':!*.test.*'
git grep -n -i -E 'testnet|friendbot|Test SDF|localhost' -- 'src/*' '.env*'
```

For each hit, ask whether the default would silently apply on mainnet if
the variable were unset. Then compare the result with the tables above.

## Follow-ups (not blockers)

- `mock-lazer`/`mock-redstone` could refuse to run on mainnet on-chain by
  checking `env.ledger().network_id()` against the mainnet passphrase hash.
  Today "never deploy to mainnet" is documentation only. Doing this changes
  their wasm hashes, so it belongs in its own change.
- Oracle: `fly.toml`'s `internal_port = 3001` relies on `PORT=3001` being set
  (the code default is `3000`). This isn't network-shaped, but it's the same
  class of silent default.
