#![no_std]

//! Polaris market contract.
//!
//! A fully-collateralized binary conditional-token market on XLM/USD,
//! settled by a Pyth Lazer price update verified on-chain.
//!
//! Design (see repo README for the full write-up):
//! - `split(amount)` locks `amount` collateral and mints `amount` YES-shares
//!   + `amount` NO-shares to the caller (1:1, always fully collateralized).
//! - `merge(amount)` is the inverse: burns equal YES+NO and returns collateral.
//! - `buy`/`sell` are split/merge composed with a constant-product swap
//!   against an on-contract YES/NO reserve pool, giving continuous pricing
//!   and one-sided exposure without ever under-collateralizing the contract.
//! - `transfer` moves share balances between addresses (the tradability
//!   primitive — a share is an internal ledger entry, not a separate token
//!   contract, so it composes with buy/sell but has no external SEP-41
//!   identity of its own in this build).
//! - `settle` verifies a Pyth Lazer update on-chain, cross-checks it
//!   on-chain against a second, genuinely independent oracle (Reflector
//!   Network — different node operators, different data pipeline, not
//!   just a different Pyth product line) before trusting it, and marks the
//!   winning side; `cancel` is the permissionless liveness backstop if no
//!   valid, mutually-agreeing update ever arrives.
//! - `redeem` pays out 1:1 collateral per winning share (or per share of
//!   either side, if cancelled).
//!
//! ## Collateral token requirements (see #79)
//!
//! This build deliberately does **not** pin `collateral` to a single
//! hard-coded asset (e.g. the native XLM SAC): the same contract is used
//! for testnet assets and for any standard SEP-41 token an admin chooses.
//! Instead the requirement is *documented and probed*: `collateral` must
//! be a standard SEP-41 token whose `transfer` moves exactly the requested
//! amount, debits the sender, credits the recipient, and returns `()`
//! (no fee-on-transfer, no rebasing, no transfer hooks that re-enter this
//! contract, no "returns the amount moved" variants). `initialize` calls
//! `decimals()` on it and rejects anything that isn't a token with
//! `0..=18` decimals (`Error::InvalidCollateral`); the rest of the
//! requirement is not checkable on-chain and is the deployer's
//! responsibility — a non-conforming token breaks the invariant below, not
//! this contract's arithmetic.
//!
//! Invariant (see tests): `collateral_token.balance(contract) ==
//! market.total_supply` holds after *every* successful call, for the whole
//! lifecycle. Because every share is minted as a matched YES+NO pair, the
//! contract can never be short of collateral to pay winners — there is no
//! "empty winning pool" edge case to special-case.
//!
//! ## Why `settle` fails closed on the Reflector check, not gracefully
//!
//! An earlier, off-chain-only version of this idea (still present in
//! `polaris-oracle`, Lazer vs. Hermes) skips itself when the second source
//! is unavailable — correct there, because it's advisory on top of an
//! already-fully-trusted signature; skipping it leaves the system exactly
//! as safe as before that check existed. Once the check is *on-chain and
//! enforced*, as it is here, "skip when inconvenient" stops being neutral:
//! it turns the real rule into "N-of-2 unless an adversary times
//! submission around a stale window," which is worse than not having the
//! check at all. So `None`/stale/divergent Reflector data all reject the
//! call outright. This doesn't strand funds — the existing permissionless
//! `cancel` after `expiry + grace_period` is already the unconditional
//! liveness backstop, so a market that can never get Reflector agreement
//! cancels and refunds through the exact same well-tested path every other
//! unresolvable market already uses, preserving the "nobody unilaterally
//! decides" property even in the failure case.
//!
//! ## Deterministic settlement price
//!
//! `settle()` is permissionless and the caller supplies the signed Lazer
//! payload, so *which* print is used must not be the caller's choice. The
//! rule: the print must be timestamped at or before `expiry`, and no more
//! than `PRICE_FRESHNESS_WINDOW_SECS` (300s) before it. Post-expiry prints
//! are rejected (`FuturePrice`) even though they are fresher — accepting
//! them would let the caller pick any print in `[expiry, expiry + 300s]`
//! and settle on whichever side of the strike they hold. `settle_via_reflector`
//! enforces the identical bound on Reflector's `lastprice` timestamp.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Bytes, Env,
    Symbol,
};

use pyth_lazer_stellar_sdk::PythLazerClient;

// See polaris-ctf-math's module doc: the CPMM/fee-curve math and the
// Reflector client are shared verbatim with polaris-perpetual now, not
// carried inline here — a pure, behavior-preserving extraction (re-run
// this crate's full test suite after touching this file; every number
// must come out identical).
use polaris_ctf_math::{
    apply_fee, cpmm_out, cpmm_sell_out, effective_fee_bps as shared_effective_fee_bps, isqrt, reflector_price_to_cents,
    sep40::Sep40Client, to_cents, verify_oracle_corroboration, OracleCheckError, OracleFeedConfig,
    MAX_STALENESS_SECS,
};

/// Protocol fee ceiling: 1000 bps = 10%.
const MAX_FEE_BPS: u32 = 1_000;
/// Oracle corroboration tolerance ceiling: 1000 bps = 10%. A tolerance at or
/// above 10_000 bps would accept any divergence at all, silently disabling
/// the second-oracle cross-check; 0 would reject every price outright.
const MAX_TOLERANCE_BPS: u32 = 1_000;
/// Settlement price must be timestamped within this many seconds of expiry.
///
/// Asymmetric on purpose (see the README's settlement section): a print
/// timestamped *before* expiry is still accepted within this window, but a
/// print timestamped *after* expiry is rejected outright. The caller of
/// `settle()` picks which signed update to submit, so accepting a
/// post-expiry print would let them choose the settlement price from any
/// print in `[expiry, expiry + 300s]` — a free option on the outcome.
/// Pre-expiry prints are not a choice: the last print at or before expiry
/// is the price the market was actually trading at when it closed.
const PRICE_FRESHNESS_WINDOW_SECS: u64 = 300;
/// Upper bound on `expiry` and on `grace_period`, so that
/// `expiry + grace_period` can never overflow `u64` — otherwise `cancel()`,
/// the permissionless liveness backstop, could trap forever and strand
/// every holder's funds. Both are capped at `u64::MAX / 2`, which leaves
/// their sum representable with room to spare.
const MAX_EXPIRY: u64 = u64::MAX / 2;
const MAX_GRACE_PERIOD: u64 = u64::MAX / 2;
/// Instance/persistent storage TTL bump (ledgers), applied on every write.
const BUMP_TO: u32 = 535_680; // ~31 days at 5s/ledger
const BUMP_THRESHOLD: u32 = 500_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidStrikePrice = 3,
    InvalidExpiry = 4,
    InvalidGracePeriod = 5,
    InvalidFeedId = 6,
    InvalidFeeBps = 7,
    InvalidLiquidity = 8,
    /// `status` is no longer `Open`: the market already resolved
    /// (`ResolvedYes`/`ResolvedNo`) or was cancelled. Nothing but `redeem`
    /// and `transfer` will ever succeed again. Was `MarketNotOpen` before
    /// #58 (same code, `9`).
    ///
    /// Not to be confused with `MarketExpiredAwaitingSettlement`, where
    /// `status` is still `Open` but `expiry` has passed.
    MarketSettledOrCancelled = 9,
    /// `buy`/`sell` after `expiry` on a market that is still `Open`, i.e.
    /// waiting for `settle`/`settle_via_reflector` (or, past the grace
    /// period, `cancel`). `split`/`merge` stay available until it is
    /// finalized. Was `TradingClosed` before #58 (same code, `10`).
    ///
    /// Not to be confused with `MarketSettledOrCancelled`, where the
    /// market is already finalized.
    MarketExpiredAwaitingSettlement = 10,
    InvalidAmount = 11,
    InsufficientBalance = 12,
    SlippageExceeded = 13,
    ExpiryNotReached = 14,
    InvalidPayload = 15,
    FeedNotFound = 16,
    /// Settlement print is more than `PRICE_FRESHNESS_WINDOW_SECS` before
    /// `expiry` — too old to be the price the market closed at.
    StalePrice = 17,
    /// Settlement print is timestamped *after* `expiry`. Rejected even
    /// within the freshness window: the caller of `settle()` chooses the
    /// payload, so a post-expiry print would let them pick the outcome.
    FuturePrice = 18,
    GracePeriodNotElapsed = 19,
    AlreadyFinalized = 20,
    NotFinalized = 21,
    NothingToRedeem = 22,
    /// A trade would consume an entire AMM reserve (or, due to integer
    /// floor division, all but a negligible remainder of it) — rejected
    /// outright rather than letting a reserve hit nearly zero and permanently
    /// break that side's pricing. See `buy`/`sell`'s pool-depth guard.
    PoolDepthExceeded = 23,
    /// Reflector's `lastprice` returned `None` for this market's asset —
    /// fails closed, see the module doc's "why fail closed" section.
    ReflectorPriceUnavailable = 24,
    /// Reflector's most recent price is older than `reflector.max_staleness_secs`.
    ReflectorPriceStale = 25,
    /// Reflector's live `decimals()` is outside `2..=30`, the range the
    /// `10^decimals` cents scaling supports without overflow/underflow.
    /// Checked at `initialize` and again on every settle. Was
    /// `ReflectorDecimalsInvalid` before #58 (same code, `26`).
    ///
    /// Not to be confused with `ReflectorDecimalsDriftedSinceInit`, which
    /// is an in-range value that differs from the one pinned at init.
    ReflectorDecimalsOutOfRange = 26,
    /// Lazer's and Reflector's prices diverge by more than
    /// `reflector.tolerance_bps` — the on-chain N-of-2 enforcement itself.
    OracleDivergence = 27,
    /// `initialize`-time validation: `reflector.max_staleness_secs` is
    /// narrower than Reflector's own update `resolution()` — a window that
    /// could never realistically be met.
    InvalidReflectorConfig = 28,
    /// Reflector's live `decimals()` no longer matches the value pinned at
    /// `initialize()` — see `polaris_ctf_math::OracleCheckError::DecimalsChanged`.
    /// Settle-time only. Was `ReflectorDecimalsChanged` before #58 (same
    /// code, `29`).
    ReflectorDecimalsDriftedSinceInit = 29,
    /// RedStone's `lastprice` returned `None` for this market's asset —
    /// same fail-closed treatment as the Reflector leg. Only reachable if
    /// this market was configured with a `redstone` oracle at `initialize`.
    RedstonePriceUnavailable = 30,
    RedstonePriceStale = 31,
    /// RedStone leg's equivalent of `ReflectorDecimalsOutOfRange`. Was
    /// `RedstoneDecimalsInvalid` before #58 (same code, `32`).
    RedstoneDecimalsOutOfRange = 32,
    /// RedStone's live `decimals()` no longer matches the value pinned at
    /// `initialize()` — the specific landmine RedStone's own SEP-40
    /// wrapper contract documents (its `decimals()` is the max across
    /// *every* asset it has registered, not this asset's own precision,
    /// and can change): see this repo's README for how this was found.
    /// Was `RedstoneDecimalsChanged` before #58 (same code, `33`).
    RedstoneDecimalsDriftedSinceInit = 33,
    /// `initialize`-time validation for the `redstone` leg, mirroring
    /// `InvalidReflectorConfig`. A RedStone-vs-Lazer divergence beyond
    /// `redstone.tolerance_bps` reuses the shared `OracleDivergence`
    /// variant above — unanimous, not majority: either leg disagreeing
    /// rejects the call the same way.
    InvalidRedstoneConfig = 34,
    /// `buy`/`sell` are blocked while the admin has paused trading — see
    /// `pause()`'s doc comment. `split`/`merge`/`redeem` are never blocked
    /// by this, only the AMM-math-dependent entrypoints a pause exists to
    /// protect against.
    Paused = 35,
    /// `transfer`'s `to` is this contract's own address — shares sent there
    /// could never be moved out again, so they'd be permanently stranded.
    InvalidRecipient = 36,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[contracttype]
pub enum Prediction {
    Yes,
    No,
}

impl Prediction {
    fn other(&self) -> Prediction {
        match self {
            Prediction::Yes => Prediction::No,
            Prediction::No => Prediction::Yes,
        }
    }
}

/// Which settlement path finalized the market — recorded in the `settle`
/// event so indexers can tell a Lazer-verified settlement from a
/// Reflector-promoted one without replaying the transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[contracttype]
pub enum SettlementSource {
    Lazer,
    Reflector,
}

/// The read-only answer from `preview_settlement` /
/// `preview_settlement_via_reflector`: what settlement would write if called
/// right now. Never stored — no market fields are touched by a preview.
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct SettlementPreview {
    /// The price settlement would finalize at, in cents.
    pub final_price: i128,
    /// `ResolvedYes` iff `final_price >= strike_price`, else `ResolvedNo`.
    /// A successful preview never returns `Open` or `Cancelled`.
    pub outcome: MarketStatus,
}

impl SettlementPreview {
    fn new(final_price: i128, strike_price: i128) -> Self {
        Self {
            final_price,
            outcome: outcome_for(final_price, strike_price),
        }
    }
}

/// The single source of truth for the price-to-outcome rule, shared by both
/// settlement paths and both previews so they cannot drift apart.
fn outcome_for(final_price: i128, strike_price: i128) -> MarketStatus {
    if final_price >= strike_price {
        MarketStatus::ResolvedYes
    } else {
        MarketStatus::ResolvedNo
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[contracttype]
pub enum MarketStatus {
    Open,
    ResolvedYes,
    ResolvedNo,
    Cancelled,
}

// `OracleFeedConfig` itself now lives in `polaris_ctf_math` (imported
// above) — settle-time corroborating-oracle settings, bundled into one
// struct rather than four more flat `initialize()` scalars, shared
// verbatim with `polaris-perpetual` and across both the Reflector and
// RedStone legs, since it means the same thing for all of them.

#[derive(Clone)]
#[contracttype]
pub struct Market {
    pub admin: Address,
    pub collateral: Address,
    pub strike_price: i128, // cents
    pub expiry: u64,
    pub grace_period: u64,
    pub lazer_contract: Address,
    pub feed_id: u32,
    pub base_fee_bps: u32, // swap fee charged when total_supply == initial_liquidity (no volume yet)
    pub min_fee_bps: u32,  // swap fee approached as total_supply grows without bound
    pub treasury: Address,
    pub status: MarketStatus,
    pub final_price: i128, // cents; 0 until resolved
    pub pool_yes: i128,    // AMM reserve; 0 once trading has ended
    pub pool_no: i128,
    pub total_supply: i128, // == total YES outstanding == total NO outstanding, pre-resolution
    pub initial_liquidity: i128, // immutable reference scale for the fee curve; total_supply >= this always, pre-resolution
    pub reflector: OracleFeedConfig,
    /// Reflector's `decimals()` as observed once, live, at `initialize()`
    /// time — re-checked on every `settle()` call so a live drift fails
    /// closed instead of silently corrupting the cents conversion. Plain
    /// (non-`Option`) field: this leg is always configured, unlike
    /// `redstone` below.
    pub reflector_decimals_at_init: u32,
}

/// A configured RedStone corroboration leg plus the `decimals()` value
/// observed, live, at `initialize()` time — bundled together since both
/// are needed on every `settle()` call and neither is meaningful alone.
/// Stored under its own `DataKey::Redstone` instance-storage entry rather
/// than as an `Option<OracleFeedConfig>` field on `Market` itself: see
/// this repo's README ("A soroban-sdk gotcha worth recording") for why
/// `Option<CustomStruct>` can't be a `#[contracttype]` struct field.
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct RedstoneOracle {
    pub config: OracleFeedConfig,
    pub decimals_at_init: u32,
}

/// `get_pool_growth()`'s answer to "how much of the AMM pool is fee yield,
/// and how much is the seed plus price movement?" (#31). Every field is
/// derived from `Market`'s existing `pool_yes`/`pool_no`/`initial_liquidity`
/// on each call. Nothing extra is stored, and the stored `Market` shape
/// is unchanged.
///
/// Why the split is clean: `buy`/`sell` without a fee would keep the
/// constant product `k = pool_yes * pool_no` fixed while moving the price.
/// Up to integer rounding, the fee is the only thing that grows `k`,
/// because the pool keeps the full input but pays out only against the
/// fee-reduced `effective_in`. So `sqrt(k)` is a price-independent measure
/// of depth, and its growth over the seed is fee accrual. What's left in
/// the pool's marked value is the price-driven part.
///
/// Exact identity, in collateral units (integer floors included):
/// `pool_value == initial_liquidity + fee_growth + price_effect`.
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct PoolGrowth {
    /// Seed collateral, which is also each reserve's starting depth.
    pub initial_liquidity: i128,
    pub pool_yes: i128,
    pub pool_no: i128,
    /// `floor(sqrt(pool_yes * pool_no))`: depth with the price effect
    /// removed. Equals `initial_liquidity` on a fresh market.
    pub invariant_liquidity: i128,
    /// `invariant_liquidity - initial_liquidity`. Fee accrual, plus the
    /// integer rounding in `cpmm_out`/`cpmm_sell_out`, which can go either
    /// way by about one unit of `sqrt(k)` per trade. With a nonzero fee the
    /// fee dominates, so this only goes negative on a market whose fee
    /// curve is at `0` bps, and then only by that rounding.
    pub fee_growth: i128,
    /// The pool's YES + NO shares marked at the current implied price, the
    /// same price `get_price` reports: `pool_yes * p_yes + pool_no * p_no`
    /// with `p_yes = pool_no / (pool_yes + pool_no)`, which simplifies to
    /// `floor(2 * pool_yes * pool_no / (pool_yes + pool_no))`.
    pub pool_value: i128,
    /// `pool_value - invariant_liquidity`: the price-driven part. Always
    /// `<= 0` (a harmonic mean never exceeds the geometric mean). This is
    /// the AMM's impermanent loss. It is `0` when the price is 50/50, so a
    /// round trip back to even odds leaves only fee growth.
    pub price_effect: i128,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Market,
    Balance(Prediction, Address),
    /// Present only if this market was `initialize`d with a `redstone`
    /// oracle configured — see `RedstoneOracle`'s doc comment.
    Redstone,
    /// Deliberately its own storage key, not a `Market` struct field — a
    /// struct field would change `Market`'s stored shape, breaking
    /// deserialization of every already-deployed instance's existing data
    /// the moment this code redeploys. A missing key just means "never
    /// paused" via `is_paused`'s `unwrap_or(false)`, so this is purely
    /// additive for old and new instances alike. See #4's identical
    /// reasoning for `vault`'s `PendingAdmin` key.
    Paused,
    /// Set only between a `propose_admin` and the matching `accept_admin` —
    /// absent the rest of the time. Own key, not a `Market` field, for the
    /// same additive-only reason as `Paused` above; mirrors `polaris-vault`'s
    /// `PendingAdmin` (#4, ported here by #86).
    PendingAdmin,
}

fn touch(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(BUMP_THRESHOLD, BUMP_TO);
}

fn load_market(env: &Env) -> Result<Market, Error> {
    env.storage()
        .instance()
        .get(&DataKey::Market)
        .ok_or(Error::NotInitialized)
}

fn save_market(env: &Env, m: &Market) {
    env.storage().instance().set(&DataKey::Market, m);
    touch(env);
}

fn load_redstone_oracle(env: &Env) -> Option<RedstoneOracle> {
    env.storage().instance().get(&DataKey::Redstone)
}

fn is_paused(env: &Env) -> bool {
    env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
}

/// Identity check first, then `require_auth` — same pattern as
/// `polaris-vault`'s `require_admin`: a mismatched caller gets a clear typed
/// error rather than an auth trap.
fn require_admin(env: &Env, admin: &Address) -> Result<(), Error> {
    let m = load_market(env)?;
    if m.admin != *admin {
        return Err(Error::Unauthorized);
    }
    admin.require_auth();
    Ok(())
}

fn balance_of(env: &Env, side: Prediction, addr: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Balance(side, addr.clone()))
        .unwrap_or(0)
}

fn set_balance(env: &Env, side: Prediction, addr: &Address, amount: i128) {
    let key = DataKey::Balance(side, addr.clone());
    if amount == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &amount);
        env.storage()
            .persistent()
            .extend_ttl(&key, BUMP_THRESHOLD, BUMP_TO);
    }
}

fn credit(env: &Env, side: Prediction, addr: &Address, amount: i128) {
    let b = balance_of(env, side, addr);
    set_balance(env, side, addr, b + amount);
}

fn debit(env: &Env, side: Prediction, addr: &Address, amount: i128) -> Result<(), Error> {
    let b = balance_of(env, side, addr);
    if b < amount {
        return Err(Error::InsufficientBalance);
    }
    set_balance(env, side, addr, b - amount);
    Ok(())
}

// cpmm_out/isqrt/cpmm_sell_out/apply_fee/to_cents/reflector_price_to_cents
// now live in polaris_ctf_math (imported above), shared verbatim with
// polaris-perpetual — see that crate's doc comment.

/// Thin wrapper preserving this file's existing `effective_fee_bps(&m)`
/// call sites unchanged — the actual curve is
/// `polaris_ctf_math::effective_fee_bps`, which takes plain scalars so
/// `polaris-perpetual` (a differently-shaped state struct) can share it too.
fn effective_fee_bps(m: &Market) -> u32 {
    shared_effective_fee_bps(m.base_fee_bps, m.min_fee_bps, m.initial_liquidity, m.total_supply)
}

fn reserves(m: &Market, side: Prediction) -> (i128, i128) {
    match side {
        Prediction::Yes => (m.pool_yes, m.pool_no),
        Prediction::No => (m.pool_no, m.pool_yes),
    }
}

fn set_reserve(m: &mut Market, side: Prediction, new_value: i128) {
    match side {
        Prediction::Yes => m.pool_yes = new_value,
        Prediction::No => m.pool_no = new_value,
    }
}

#[contract]
pub struct PolarisMarket;

#[contractimpl]
impl PolarisMarket {
    /// One-time setup: fixes every market parameter for this instance's
    /// whole life and seeds the AMM. Requires `admin`'s auth, and moves
    /// `initial_liquidity` of `collateral` from `admin` into this contract
    /// (`pool_yes == pool_no == total_supply == initial_liquidity`, status
    /// `Open`). There is no re-initialize or reconfigure path: a second
    /// call returns `AlreadyInitialized`, and a wrong parameter means
    /// deploying a fresh instance.
    ///
    /// Parameters:
    /// - `admin` — funds the seed liquidity; the only address that can
    ///   `pause`/`unpause`.
    /// - `collateral` — the SEP-41 token every share is backed by 1:1
    ///   (native XLM SAC in this build — see the module doc).
    /// - `strike_price` — in cents; resolves `ResolvedYes` iff the settled
    ///   price is `>= strike_price`.
    /// - `expiry` — unix seconds; `buy`/`sell` close and `settle` opens here.
    /// - `grace_period` — seconds after `expiry` before `cancel` (the
    ///   permissionless liveness backstop) becomes callable.
    /// - `lazer_contract`, `feed_id` — the Pyth Lazer verifier contract and
    ///   the feed `settle` reads (see the README's "Feed ID" section).
    /// - `base_fee_bps`, `min_fee_bps` — the fee curve's endpoints: a fresh
    ///   market charges `base_fee_bps`, decaying toward `min_fee_bps` as
    ///   `total_supply` grows (see `get_fee`).
    /// - `treasury` — receives the AMM pool's leftover shares at
    ///   settlement/cancellation (a `polaris-vault` address, in practice).
    /// - `initial_liquidity` — seed collateral, and the fixed reference
    ///   scale for the fee curve.
    /// - `reflector` — the always-required SEP-40 corroboration leg.
    /// - `redstone` — the optional second SEP-40 leg (`None` on testnet).
    ///   When present, corroboration is unanimous, never 2-of-3.
    ///
    /// Rejected combinations, checked in this order (the first failing
    /// check is the error returned, and nothing is stored or transferred):
    /// - `AlreadyInitialized` — this instance already holds a `Market`.
    /// - `InvalidStrikePrice` — `strike_price <= 0`: no real price could
    ///   ever resolve below it, so the outcome would be fixed in advance.
    /// - `InvalidExpiry` — `expiry` is not strictly in the future: the
    ///   market would be settleable before anyone could trade it.
    /// - `InvalidGracePeriod` — `grace_period == 0`: `cancel` would become
    ///   callable the same instant `settle` does, letting anyone void the
    ///   market in a race against the honest settlement.
    /// - `InvalidFeedId` — `feed_id == 0`, the zero value of an unset
    ///   field rather than a real Lazer feed.
    /// - `InvalidFeeBps` — `base_fee_bps > MAX_FEE_BPS` (1000 bps = 10%),
    ///   or `min_fee_bps > base_fee_bps` (the curve would rise with volume
    ///   instead of decaying).
    /// - `InvalidLiquidity` — `initial_liquidity <= 0`: an empty pool has
    ///   no constant-product price at all.
    /// - `InvalidReflectorConfig` / `InvalidRedstoneConfig` — that leg's
    ///   `max_staleness_secs` is narrower than the oracle's own live
    ///   `resolution()`, so every settle attempt would be stale.
    /// - `ReflectorDecimalsOutOfRange` / `RedstoneDecimalsOutOfRange` — that
    ///   leg's live `decimals()` is outside `2..=30`. The value read here
    ///   is pinned and re-checked on every `settle` (see
    ///   `ReflectorDecimalsDriftedSinceInit`/`RedstoneDecimalsDriftedSinceInit`).
    ///
    /// Not validated here, and failing as a trap rather than a typed
    /// error: an oracle `contract` that doesn't implement SEP-40's
    /// `resolution()`/`decimals()`, and an `admin` without
    /// `initial_liquidity` of `collateral` to transfer. `tolerance_bps`,
    /// `lazer_contract` and `treasury` are stored as given.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        env: Env,
        admin: Address,
        collateral: Address,
        strike_price: i128,
        expiry: u64,
        grace_period: u64,
        lazer_contract: Address,
        feed_id: u32,
        base_fee_bps: u32,
        min_fee_bps: u32,
        treasury: Address,
        initial_liquidity: i128,
        reflector: OracleFeedConfig,
        // Genuinely independent of the same-shape `reflector` leg —
        // RedStone's SEP-40 wrapper on Stellar (confirmed live, see this
        // repo's README). `None` on testnet, where no RedStone contract
        // exists yet — see `RedstoneOracle`'s doc comment for why this
        // can't be a `Market` struct field. When present, corroboration
        // is unanimous: this leg disagreeing rejects `settle()` exactly
        // like the Reflector leg does, never a 2-of-3 majority.
        redstone: Option<OracleFeedConfig>,
    ) -> Result<(), Error> {
        admin.require_auth();

        if env.storage().instance().has(&DataKey::Market) {
            return Err(Error::AlreadyInitialized);
        }
        if strike_price <= 0 {
            return Err(Error::InvalidStrikePrice);
        }
        if expiry <= env.ledger().timestamp() || expiry > MAX_EXPIRY {
            return Err(Error::InvalidExpiry);
        }
        if grace_period == 0 || grace_period > MAX_GRACE_PERIOD {
            return Err(Error::InvalidGracePeriod);
        }
        if feed_id == 0 {
            return Err(Error::InvalidFeedId);
        }
        if base_fee_bps > MAX_FEE_BPS || min_fee_bps > base_fee_bps {
            return Err(Error::InvalidFeeBps);
        }
        if initial_liquidity <= 0 {
            return Err(Error::InvalidLiquidity);
        }
        // See the module doc's "Collateral token requirements": the
        // solvency invariant assumes standard SEP-41 transfer semantics,
        // which can't be fully verified on-chain — but a `decimals()` probe
        // rejects the common non-conforming cases (a plain account, a
        // contract with no token interface, absurd precision) before any
        // liquidity is pulled.
        let collateral_decimals = token::Client::new(&env, &collateral).decimals();
        if collateral_decimals > 18 {
            return Err(Error::InvalidCollateral);
        }
        // An admin can't configure a staleness window narrower than
        // Reflector's own update cadence allows — that would reject every
        // settle attempt outright, not just unlucky ones.
        // ...and can't exceed `MAX_STALENESS_SECS`: the staleness check adds
        // this to an oracle-supplied timestamp, so an unbounded window could
        // overflow `u64` and trap every settle.
        let reflector_client = Sep40Client::new(&env, &reflector.contract);
        if reflector.max_staleness_secs > MAX_STALENESS_SECS
            || reflector.max_staleness_secs < reflector_client.resolution() as u64
        {
            return Err(Error::InvalidReflectorConfig);
        }
        // A tolerance of 0 rejects every price, and one above
        // `MAX_TOLERANCE_BPS` (or at/above 10_000 bps) accepts any
        // divergence — either way the corroboration check is disabled.
        if reflector.tolerance_bps == 0 || reflector.tolerance_bps > MAX_TOLERANCE_BPS {
            return Err(Error::InvalidReflectorConfig);
        }
        // Pinned once here, re-checked on every `settle()` call — see
        // `Market::reflector_decimals_at_init`'s doc comment.
        let reflector_decimals_at_init = reflector_client.decimals();
        if !(2..=30).contains(&reflector_decimals_at_init) {
            return Err(Error::ReflectorDecimalsOutOfRange);
        }
        // Fail closed at init, not at first settle: an oracle that has no
        // price for the configured asset can never settle this market.
        if reflector_client.lastprice(reflector.asset.clone()).is_none() {
            return Err(Error::ReflectorPriceUnavailable);
        }

        if let Some(cfg) = &redstone {
            let redstone_client = Sep40Client::new(&env, &cfg.contract);
            if cfg.max_staleness_secs > MAX_STALENESS_SECS
                || cfg.max_staleness_secs < redstone_client.resolution() as u64
            {
                return Err(Error::InvalidRedstoneConfig);
            }
            if cfg.tolerance_bps == 0 || cfg.tolerance_bps > MAX_TOLERANCE_BPS {
                return Err(Error::InvalidRedstoneConfig);
            }
            let redstone_decimals_at_init = redstone_client.decimals();
            if redstone_client.lastprice(cfg.asset.clone()).is_none() {
                return Err(Error::RedstonePriceUnavailable);
            }
            if !(2..=30).contains(&redstone_decimals_at_init) {
                return Err(Error::RedstoneDecimalsOutOfRange);
            }
            env.storage().instance().set(
                &DataKey::Redstone,
                &RedstoneOracle { config: cfg.clone(), decimals_at_init: redstone_decimals_at_init },
            );
        }

        token::Client::new(&env, &collateral).transfer(
            &admin,
            &env.current_contract_address(),
            &initial_liquidity,
        );

        let market = Market {
            admin,
            collateral,
            strike_price,
            expiry,
            grace_period,
            lazer_contract,
            feed_id,
            base_fee_bps,
            min_fee_bps,
            treasury,
            status: MarketStatus::Open,
            reflector,
            reflector_decimals_at_init,
            final_price: 0,
            pool_yes: initial_liquidity,
            pool_no: initial_liquidity,
            total_supply: initial_liquidity,
            initial_liquidity,
        };
        save_market(&env, &market);
        // Every immutable parameter an indexer needs to interpret this
        // market's later events, so it never has to call `get_market()` to
        // bootstrap. Pools and `total_supply` all start at
        // `initial_liquidity`. See the README's "Event schema" section.
        env.events().publish(
            (symbol_short!("init"),),
            (
                market.strike_price,
                market.expiry,
                market.grace_period,
                market.feed_id,
                market.base_fee_bps,
                market.min_fee_bps,
                market.initial_liquidity,
                market.admin,
                market.treasury,
                market.collateral,
            ),
        );
        Ok(())
    }

    /// Admin-gated circuit breaker for `buy`/`sell` specifically — the two
    /// entrypoints whose correctness depends on the shared AMM/fee-curve
    /// math (`cpmm_out`/`cpmm_sell_out`/`apply_fee`). Independent of
    /// `status`: a paused-but-`Open` market still allows `split`/`merge`
    /// (fixed 1:1 collateral<->share pairs, not priced by the AMM, so a
    /// bug in the AMM math can't reach them) and always allows `redeem`
    /// once resolved/cancelled, so funds are never frozen by a pause. See
    /// the top-level README's pre-mainnet checklist for why this exists:
    /// before this, a live AMM-math bug had no faster mitigation than
    /// waiting for natural expiry.
    pub fn pause(env: Env) -> Result<(), Error> {
        let m = load_market(&env)?;
        require_admin(&env, &m.admin)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        touch(&env);
        env.events().publish((symbol_short!("paused"),), ());
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), Error> {
        let m = load_market(&env)?;
        require_admin(&env, &m.admin)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        touch(&env);
        env.events().publish((symbol_short!("unpaused"),), ());
        Ok(())
    }

    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Step 1 of a two-step admin rotation — ported verbatim from
    /// `polaris-vault` (#4, see #86). Deliberately not a single-call
    /// `set_admin`: a typo'd/unreachable `proposed` address would
    /// permanently brick the market out of its admin-gated entrypoints, so
    /// the new admin has to actually show up and sign `accept_admin` with
    /// its own key before the swap takes effect — a bad proposal is
    /// harmless and reversible (just propose again).
    pub fn propose_admin(env: Env, current_admin: Address, proposed: Address) -> Result<(), Error> {
        require_admin(&env, &current_admin)?;
        env.storage().instance().set(&DataKey::PendingAdmin, &proposed);
        touch(&env);
        env.events().publish((symbol_short!("propose"),), proposed.clone());
        Ok(())
    }

    /// Step 2: the proposed admin confirms with their own signature. Only
    /// the exact address `propose_admin` named can call this successfully —
    /// not the current admin, not anyone else — which is what actually
    /// proves the new key is live, held, and able to sign before it becomes
    /// the only key that can pause/unpause.
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        let pending: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(Error::NoPendingAdmin)?;
        if pending != new_admin {
            return Err(Error::Unauthorized);
        }
        new_admin.require_auth();
        let mut m = load_market(&env)?;
        m.admin = new_admin.clone();
        save_market(&env, &m);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        touch(&env);
        env.events().publish((symbol_short!("newadmin"),), new_admin);
        Ok(())
    }

    pub fn get_pending_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::PendingAdmin)
    }

    pub fn get_admin(env: Env) -> Result<Address, Error> {
        Ok(load_market(&env)?.admin)
    }

    /// Lock `amount` collateral, mint `amount` YES + `amount` NO to `user`.
    pub fn split(env: Env, user: Address, amount: i128) -> Result<(), Error> {
        user.require_auth();
        let mut m = load_market(&env)?;
        if m.status != MarketStatus::Open {
            return Err(Error::MarketSettledOrCancelled);
        }
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        token::Client::new(&env, &m.collateral).transfer(
            &user,
            &env.current_contract_address(),
            &amount,
        );
        credit(&env, Prediction::Yes, &user, amount);
        credit(&env, Prediction::No, &user, amount);
        m.total_supply += amount;
        save_market(&env, &m);
        env.events()
            .publish((symbol_short!("split"), user), (amount, m.total_supply));
        Ok(())
    }

    /// Burn equal YES+NO from `user`, return `amount` collateral.
    pub fn merge(env: Env, user: Address, amount: i128) -> Result<(), Error> {
        user.require_auth();
        let mut m = load_market(&env)?;
        if m.status != MarketStatus::Open {
            return Err(Error::MarketSettledOrCancelled);
        }
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        debit(&env, Prediction::Yes, &user, amount)?;
        debit(&env, Prediction::No, &user, amount)?;

        token::Client::new(&env, &m.collateral).transfer(
            &env.current_contract_address(),
            &user,
            &amount,
        );
        m.total_supply -= amount;
        save_market(&env, &m);
        env.events()
            .publish((symbol_short!("merge"), user), (amount, m.total_supply));
        Ok(())
    }

    /// Split `collateral_amount`, then swap the unwanted side into more of
    /// `prediction` via the AMM. Returns total `prediction`-shares now held
    /// from this call (`collateral_amount` from the split + the swap bonus).
    pub fn buy(
        env: Env,
        user: Address,
        prediction: Prediction,
        collateral_amount: i128,
        min_shares_out: i128,
    ) -> Result<i128, Error> {
        user.require_auth();
        if is_paused(&env) {
            return Err(Error::Paused);
        }
        let mut m = load_market(&env)?;
        if m.status != MarketStatus::Open {
            return Err(Error::MarketSettledOrCancelled);
        }
        if env.ledger().timestamp() >= m.expiry {
            return Err(Error::MarketExpiredAwaitingSettlement);
        }
        if collateral_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let unwanted = prediction.other();
        let (reserve_in, reserve_out) = reserves(&m, unwanted);
        let fee_bps = effective_fee_bps(&m);
        let effective_in = apply_fee(collateral_amount, fee_bps);
        let amount_out = cpmm_out(reserve_in, reserve_out, effective_in);
        // Cap single-trade impact to half the reserve being drawn from. Not
        // just a slippage nicety: cpmm_out's integer floor division lets a
        // large enough trade against a shallow pool claim nearly the whole
        // reserve — a 10.1 XLM buy against a 10_000-stroop seeded pool once
        // drained it from 10_000 down to 1 in this build's own test suite.
        // `min_shares_out` doesn't catch this (the caller can set it to 0,
        // and did, by default); the contract needs its own floor.
        if amount_out * 2 >= reserve_out {
            return Err(Error::PoolDepthExceeded);
        }
        if amount_out < min_shares_out {
            return Err(Error::SlippageExceeded);
        }

        token::Client::new(&env, &m.collateral).transfer(
            &user,
            &env.current_contract_address(),
            &collateral_amount,
        );
        credit(&env, Prediction::Yes, &user, collateral_amount);
        credit(&env, Prediction::No, &user, collateral_amount);
        m.total_supply += collateral_amount;

        debit(&env, unwanted, &user, collateral_amount)?;
        set_reserve(&mut m, unwanted, reserve_in + collateral_amount);
        set_reserve(&mut m, prediction, reserve_out - amount_out);
        credit(&env, prediction, &user, amount_out);

        save_market(&env, &m);
        // Leading two fields unchanged, so older positional decoders still
        // work. The appended fee rate and post-trade pools/supply let an
        // indexer show the fee and price without reading `get_market()`.
        env.events().publish(
            (symbol_short!("buy"), user, prediction),
            (collateral_amount, amount_out, fee_bps, m.pool_yes, m.pool_no, m.total_supply),
        );
        Ok(collateral_amount + amount_out)
    }

    /// Sell `shares_in` of `prediction` directly for collateral in one call
    /// (see `cpmm_sell_out` for why this needs its own formula rather than
    /// "swap to the opposite side, then merge").
    pub fn sell(
        env: Env,
        user: Address,
        prediction: Prediction,
        shares_in: i128,
        min_collateral_out: i128,
    ) -> Result<i128, Error> {
        user.require_auth();
        if is_paused(&env) {
            return Err(Error::Paused);
        }
        let mut m = load_market(&env)?;
        if m.status != MarketStatus::Open {
            return Err(Error::MarketSettledOrCancelled);
        }
        if env.ledger().timestamp() >= m.expiry {
            return Err(Error::MarketExpiredAwaitingSettlement);
        }
        if shares_in <= 0 {
            return Err(Error::InvalidAmount);
        }
        if balance_of(&env, prediction, &user) < shares_in {
            return Err(Error::InsufficientBalance);
        }

        let opposite = prediction.other();
        let (reserve_in, reserve_out) = reserves(&m, prediction);
        let fee_bps = effective_fee_bps(&m);
        let effective_in = apply_fee(shares_in, fee_bps);
        let collateral_out = cpmm_sell_out(reserve_in, reserve_out, effective_in);
        // Same reserve-depth cap as `buy`, against the same reserve
        // (`reserve_out`, the opposite side's pool — the one this trade only
        // ever *decreases*, unlike `reserve_in` which is inflated by
        // `shares_in` first and so has more headroom).
        if collateral_out * 2 >= reserve_out {
            return Err(Error::PoolDepthExceeded);
        }
        if collateral_out <= 0 || collateral_out < min_collateral_out {
            return Err(Error::SlippageExceeded);
        }

        debit(&env, prediction, &user, shares_in)?;
        set_reserve(&mut m, prediction, reserve_in + shares_in - collateral_out);
        set_reserve(&mut m, opposite, reserve_out - collateral_out);

        token::Client::new(&env, &m.collateral).transfer(
            &env.current_contract_address(),
            &user,
            &collateral_out,
        );
        m.total_supply -= collateral_out;

        save_market(&env, &m);
        env.events().publish(
            (symbol_short!("sell"), user, prediction),
            (shares_in, collateral_out, fee_bps, m.pool_yes, m.pool_no, m.total_supply),
        );
        Ok(collateral_out)
    }

    /// Move a share balance between addresses — the tradability primitive.
    pub fn transfer(
        env: Env,
        from: Address,
        to: Address,
        prediction: Prediction,
        amount: i128,
    ) -> Result<(), Error> {
        from.require_auth();
        load_market(&env)?; // must be initialized
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        // Shares sent to the contract itself could never be moved out again
        // (no entrypoint spends the contract's own share balance), so they'd
        // be permanently stranded — reject outright.
        if to == env.current_contract_address() {
            return Err(Error::InvalidRecipient);
        }
        // Self-transfer is a deliberate no-op: it can't change any balance,
        // so it succeeds without touching storage or emitting an event.
        if from == to {
            return Ok(());
        }
        debit(&env, prediction, &from, amount)?;
        credit(&env, prediction, &to, amount);
        env.events()
            .publish((symbol_short!("xfer"), from, to, prediction), amount);
        Ok(())
    }

    /// Permissionlessly settle with an oracle-signed payload once expiry has passed.
    pub fn settle(env: Env, payload: Bytes) -> Result<(), Error> {
        let mut m = load_market(&env)?;
        Self::check_settlement_ready(&env, &m)?;
        let final_cents = Self::lazer_settlement_price(&env, &m, &payload)?;
        Self::finalize_settlement(&env, &mut m, final_cents, SettlementSource::Lazer);
        Ok(())
    }

    /// Read-only counterpart of `settle`: the outcome it would produce using
    /// the caller's signed Lazer payload, without finalizing or emitting any
    /// event. Runs the exact same readiness, verification, freshness and
    /// corroboration checks as `settle` (via the shared helpers), so a
    /// successful preview is a promise about what `settle(payload)` would do;
    /// an `Err` is the same reason `settle` would fail right now.
    pub fn preview_settlement(env: Env, payload: Bytes) -> Result<SettlementPreview, Error> {
        let m = load_market(&env)?;
        Self::check_settlement_ready(&env, &m)?;
        Ok(SettlementPreview::new(
            Self::lazer_settlement_price(&env, &m, &payload)?,
            m.strike_price,
        ))
    }

    /// Read-only counterpart of `settle_via_reflector`, for frontends that
    /// have no signed Lazer payload. Same shared logic, zero state change.
    pub fn preview_settlement_via_reflector(env: Env) -> Result<SettlementPreview, Error> {
        let m = load_market(&env)?;
        Self::check_settlement_ready(&env, &m)?;
        Ok(SettlementPreview::new(
            Self::reflector_settlement_price(&env, &m)?,
            m.strike_price,
        ))
    }

    /// Preconditions common to both settlement paths and both previews.
    fn check_settlement_ready(env: &Env, m: &Market) -> Result<(), Error> {
        if m.status != MarketStatus::Open {
            return Err(Error::AlreadyFinalized);
        }
        if env.ledger().timestamp() < m.expiry {
            return Err(Error::ExpiryNotReached);
        }
        Ok(())
    }

    /// Lazer verification + freshness + all corroboration legs, extracted
    /// verbatim from `settle`. Returns the would-be final price in cents;
    /// mutation happens only in `finalize_settlement`.
    fn lazer_settlement_price(env: &Env, m: &Market, payload: &Bytes) -> Result<i128, Error> {
        let now = env.ledger().timestamp();
        let client = PythLazerClient::new(env, &m.lazer_contract);
        let verified = client
            .verify_update(payload)
            .map_err(|_| Error::InvalidPayload)?;

        let feed = verified
            .feeds
            .iter()
            .find(|f| f.feed_id == m.feed_id)
            .ok_or(Error::FeedNotFound)?;
        let price = feed.price.ok_or(Error::InvalidPayload)?;
        if price <= 0 {
            return Err(Error::InvalidPayload);
        }
        let exponent = feed.exponent.ok_or(Error::InvalidPayload)?;
        let feed_ts_micros = feed.feed_update_timestamp.unwrap_or(verified.timestamp);
        let feed_ts_secs = feed_ts_micros / 1_000_000;

        if feed_ts_secs.saturating_add(PRICE_FRESHNESS_WINDOW_SECS) < m.expiry {
            return Err(Error::StalePrice);
        }
        if feed_ts_secs > m.expiry.saturating_add(PRICE_FRESHNESS_WINDOW_SECS) {
            return Err(Error::FuturePrice);
        }

        let final_cents = to_cents(price, exponent);

        // On-chain second- (and, if configured, third-) oracle enforcement
        // — see the module doc's "why fail closed" section. No mutation of
        // `m` has happened yet at this point, so every early return below
        // is a clean revert. Unanimous: any configured leg disagreeing or
        // unavailable rejects the whole call, never a majority vote.
        verify_oracle_corroboration(env, &m.reflector, m.reflector_decimals_at_init, now, final_cents).map_err(
            |e| match e {
                OracleCheckError::Unavailable => Error::ReflectorPriceUnavailable,
                OracleCheckError::Stale => Error::ReflectorPriceStale,
                OracleCheckError::DecimalsInvalid => Error::ReflectorDecimalsOutOfRange,
                OracleCheckError::DecimalsChanged => Error::ReflectorDecimalsDriftedSinceInit,
                OracleCheckError::FutureDated => Error::ReflectorPriceFutureDated,
                OracleCheckError::Divergent => Error::OracleDivergence,
            },
        )?;
        if let Some(redstone) = load_redstone_oracle(env) {
            verify_oracle_corroboration(env, &redstone.config, redstone.decimals_at_init, now, final_cents)
                .map_err(Self::map_redstone_error)?;
        }

        Ok(final_cents)
    }

    /// A second, independent settlement path for when no Pyth Lazer
    /// payload is available at all — not a degraded fallback, a genuinely
    /// real one: Reflector Network is itself a live, decentralized,
    /// on-chain price oracle, the same one `settle()` has always required
    /// to *corroborate* Lazer before finalizing. This entrypoint just
    /// promotes it from corroborator to the reference price directly,
    /// reusing the exact same freshness/decimals-pin discipline that path
    /// already enforces — see `polaris-oracle`'s README for why this
    /// exists (Pyth Lazer's public access now requires a paid
    /// subscription; Reflector is free and was already trusted equally).
    ///
    /// Deliberately a **separate function**, not an `Option<Bytes>`
    /// parameter on `settle()` itself: the two paths verify genuinely
    /// different things (a cryptographic payload signature vs. a live
    /// contract read), and collapsing them into one entrypoint with a
    /// branch would make it easy to silently accept whichever path is
    /// convenient rather than whichever was actually intended by the
    /// caller. Same preconditions, same payout mechanics, same fail-closed
    /// behavior on stale/unavailable/drifted data as the Lazer path.
    pub fn settle_via_reflector(env: Env) -> Result<(), Error> {
        let mut m = load_market(&env)?;
        Self::check_settlement_ready(&env, &m)?;
        let final_cents = Self::reflector_settlement_price(&env, &m)?;
        Self::finalize_settlement(&env, &mut m, final_cents, SettlementSource::Reflector);
        Ok(())
    }

    /// Reflector read + freshness + decimals-pin + RedStone corroboration,
    /// extracted verbatim from `settle_via_reflector`. Returns the would-be
    /// final price in cents; no state is touched until `finalize_settlement`.
    fn reflector_settlement_price(env: &Env, m: &Market) -> Result<i128, Error> {
        let now = env.ledger().timestamp();
        let client = Sep40Client::new(env, &m.reflector.contract);
        let price = client.lastprice(&m.reflector.asset).ok_or(Error::ReflectorPriceUnavailable)?;
        // Saturating, not `+`: `price.timestamp` is oracle-supplied and can
        // sit near `u64::MAX`, which would otherwise trap the whole call.
        if price.timestamp.saturating_add(m.reflector.max_staleness_secs) < now {
            return Err(Error::ReflectorPriceStale);
        }
        if price.timestamp > now + polaris_ctf_math::MAX_FUTURE_SKEW_SECS {
            return Err(Error::ReflectorPriceFutureDated);
        }
        // Same "timestamped near expiry, not just not-stale-right-now"
        // bound the Lazer path enforces via PRICE_FRESHNESS_WINDOW_SECS —
        // a price that's merely within max_staleness_secs of `now` could
        // still predate `expiry` by an unbounded amount if settle() is
        // called long after expiry on a stalled feed.
        if price.timestamp.saturating_add(PRICE_FRESHNESS_WINDOW_SECS) < m.expiry {
            return Err(Error::StalePrice);
        }
        // Upper bound mirroring the Lazer path exactly: a price timestamped
        // after expiry is not a settlement price for this market, so reject
        // it rather than settling on a post-expiry print the caller chose.
        if price.timestamp > m.expiry {
            return Err(Error::FuturePrice);
        }
        let decimals = client.decimals();
        if decimals != m.reflector_decimals_at_init {
            return Err(Error::ReflectorDecimalsDriftedSinceInit);
        }
        let final_cents = reflector_price_to_cents(price.price, decimals);

        // If a third leg (RedStone) is configured, it must still agree —
        // unanimous, not "Reflector alone is always enough once it's
        // promoted to primary." Mirrors settle()'s own RedStone check
        // exactly, just corroborating against Reflector's price instead
        // of Lazer's.
        if let Some(redstone) = load_redstone_oracle(env) {
            verify_oracle_corroboration(env, &redstone.config, redstone.decimals_at_init, now, final_cents)
                .map_err(Self::map_redstone_error)?;
        }

        Ok(final_cents)
    }

    /// Maps the optional RedStone leg's `OracleCheckError` to this
    /// contract's `Error` enum. Shared by `settle()` and
    /// `settle_via_reflector()`, which both corroborate against the same
    /// optional RedStone oracle and need the identical mapping.
    fn map_redstone_error(e: OracleCheckError) -> Error {
        match e {
            OracleCheckError::Unavailable => Error::RedstonePriceUnavailable,
            OracleCheckError::Stale => Error::RedstonePriceStale,
            OracleCheckError::DecimalsInvalid => Error::RedstoneDecimalsOutOfRange,
            OracleCheckError::DecimalsChanged => Error::RedstoneDecimalsDriftedSinceInit,
            OracleCheckError::FutureDated => Error::RedstonePriceFutureDated,
            OracleCheckError::Divergent => Error::OracleDivergence,
        }
    }

    /// Shared tail of both settlement paths (`settle`/`settle_via_reflector`)
    /// — once a final price has cleared every configured oracle check,
    /// finalizing is identical regardless of which path produced it.
    fn finalize_settlement(
        env: &Env,
        m: &mut Market,
        final_cents: i128,
        source: SettlementSource,
    ) {
        m.final_price = final_cents;
        m.status = outcome_for(final_cents, m.strike_price);

        let (swept_yes, swept_no) = (m.pool_yes, m.pool_no);
        credit(env, Prediction::Yes, &m.treasury, swept_yes);
        credit(env, Prediction::No, &m.treasury, swept_no);
        m.pool_yes = 0;
        m.pool_no = 0;

        save_market(env, m);
        env.events()
            .publish((symbol_short!("settle"),), (m.status, final_cents, source));
    }

    /// Permissionless liveness backstop: if nobody ever settles, anyone can
    /// cancel after `expiry + grace_period`. Both sides then redeem 1:1.
    pub fn cancel(env: Env) -> Result<(), Error> {
        let mut m = load_market(&env)?;
        if m.status != MarketStatus::Open {
            return Err(Error::AlreadyFinalized);
        }
        let now = env.ledger().timestamp();
        // Saturating: `initialize` already bounds both terms so this can't
        // overflow, but the backstop must never trap even for a market
        // stored before that bound existed.
        if now < m.expiry.saturating_add(m.grace_period) {
            return Err(Error::GracePeriodNotElapsed);
        }

        let (swept_yes, swept_no) = (m.pool_yes, m.pool_no);
        credit(&env, Prediction::Yes, &m.treasury, swept_yes);
        credit(&env, Prediction::No, &m.treasury, swept_no);
        m.pool_yes = 0;
        m.pool_no = 0;
        m.status = MarketStatus::Cancelled;

        save_market(&env, &m);
        env.events()
            .publish((symbol_short!("cancel"),), (m.treasury.clone(), swept_yes, swept_no));
        Ok(())
    }

    /// Redeem `user`'s position after resolution or cancellation.
    pub fn redeem(env: Env, user: Address) -> Result<i128, Error> {
        user.require_auth();
        let mut m = load_market(&env)?;

        // Every finalized branch below zeroes both balances, so record what
        // is being burned first. The `redeem` event carries both, which lets an
        // indexer show exactly which shares were redeemed.
        let by = balance_of(&env, Prediction::Yes, &user);
        let bn = balance_of(&env, Prediction::No, &user);

        let payout = match m.status {
            MarketStatus::Open => return Err(Error::NotFinalized),
            MarketStatus::ResolvedYes => {
                set_balance(&env, Prediction::Yes, &user, 0);
                set_balance(&env, Prediction::No, &user, 0);
                by
            }
            MarketStatus::ResolvedNo => {
                set_balance(&env, Prediction::Yes, &user, 0);
                set_balance(&env, Prediction::No, &user, 0);
                bn
            }
            MarketStatus::Cancelled => {
                // NOT `by + bn`: `sum(all YES balances) == total_supply` and
                // `sum(all NO balances) == total_supply` are both
                // independently true (same number) — real collateral only
                // backs ONE total_supply's worth, not two. Paying `by + bn`
                // to every holder double-counts and can insolvency-lock
                // whoever redeems last (confirmed live: a plain matched
                // split/cancel/redeem already overpaid 2x, then the
                // treasury's own ordinary pool-seeded redemption failed
                // outright — "balance is not sufficient to spend"). Paying
                // each complementary token 0.5 is the standard answer for a
                // voided market in CTF-style systems generally (Polymarket,
                // Gnosis) for exactly this reason: it's the only per-holder
                // formula where summing every payout is *guaranteed* to
                // equal total_supply exactly, regardless of trading
                // history — a directional bettor's AMM-subsidized bonus
                // shares are worth less than face value once nobody's
                // collateral is "eligible" to originate from resolution.
                set_balance(&env, Prediction::Yes, &user, 0);
                set_balance(&env, Prediction::No, &user, 0);
                (by + bn) / 2
            }
        };

        if payout <= 0 {
            return Err(Error::NothingToRedeem);
        }

        token::Client::new(&env, &m.collateral).transfer(
            &env.current_contract_address(),
            &user,
            &payout,
        );
        m.total_supply -= payout;
        save_market(&env, &m);
        env.events()
            .publish((symbol_short!("redeem"), user), (payout, by, bn, m.total_supply));
        Ok(payout)
    }

    pub fn get_market(env: Env) -> Result<Market, Error> {
        load_market(&env)
    }

    /// `None` if this market was never configured with a RedStone leg —
    /// see `RedstoneOracle`'s doc comment for why this lives outside
    /// `Market` itself.
    pub fn get_redstone_oracle(env: Env) -> Option<RedstoneOracle> {
        load_redstone_oracle(&env)
    }

    pub fn get_position(env: Env, addr: Address) -> (i128, i128) {
        (
            balance_of(&env, Prediction::Yes, &addr),
            balance_of(&env, Prediction::No, &addr),
        )
    }

    /// Implied YES/NO probability in bps (sums to ~10000), from AMM reserves.
    pub fn get_price(env: Env) -> Result<(u32, u32), Error> {
        let m = load_market(&env)?;
        if m.status != MarketStatus::Open {
            return Err(Error::MarketSettledOrCancelled);
        }
        let total = m.pool_yes + m.pool_no;
        let yes_bps = (m.pool_no * 10_000 / total) as u32;
        Ok((yes_bps, 10_000 - yes_bps))
    }

    /// Current swap fee (bps), per the volume-scaled curve in `effective_fee_bps` —
    /// `base_fee_bps` when the market is fresh, decaying toward `min_fee_bps`
    /// as `total_supply` grows. Authoritative source for what `buy`/`sell`
    /// will actually charge right now; callers shouldn't recompute the curve
    /// themselves against a possibly-stale `total_supply` read elsewhere.
    pub fn get_fee(env: Env) -> Result<u32, Error> {
        let m = load_market(&env)?;
        Ok(effective_fee_bps(&m))
    }

    /// Splits the AMM pool's current size into the seed, fee-accrued
    /// growth, and the price-driven part (#31). See `PoolGrowth` for the
    /// definitions and the identity they satisfy. Read-only; it doesn't
    /// need per-share LP accounting, because it describes the whole pool,
    /// which today has one LP (the treasury/vault).
    ///
    /// Open markets only, the same as `get_price`: `settle`/`cancel` sweep
    /// both reserves to the treasury and zero them, so there is no pool
    /// left to describe. Returns `MarketSettledOrCancelled` then. The swept
    /// amounts are exactly the treasury's credited YES/NO shares (and are
    /// in the `cancel` event), so the same formulas applied to them give
    /// the pool's final breakdown.
    ///
    /// Arithmetic is the same magnitude class as `buy`/`sell`'s own
    /// (`pool_yes * pool_no` must fit in `i128`), so any pool those
    /// entrypoints can produce is covered.
    pub fn get_pool_growth(env: Env) -> Result<PoolGrowth, Error> {
        let m = load_market(&env)?;
        if m.status != MarketStatus::Open {
            return Err(Error::MarketSettledOrCancelled);
        }
        let product = m.pool_yes * m.pool_no;
        let invariant_liquidity = isqrt(product);
        let pool_value = 2 * product / (m.pool_yes + m.pool_no);
        Ok(PoolGrowth {
            initial_liquidity: m.initial_liquidity,
            pool_yes: m.pool_yes,
            pool_no: m.pool_no,
            invariant_liquidity,
            fee_growth: invariant_liquidity - m.initial_liquidity,
            pool_value,
            price_effect: pool_value - invariant_liquidity,
        })
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod proptest;
