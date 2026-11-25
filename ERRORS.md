# Error-code reference

Every Polaris contract returns errors as `Error(Contract, #N)`, where `N` is
the numeric value of the contract's `Error` enum variant. **The same numeric
code means different things in different contracts** — always resolve a code
against the table for the contract that emitted it.

## Stability policy

Error codes are **append-only**. Existing numeric values are never renumbered
or reused, even if a variant is renamed or deprecated. New variants are added
with the next unused number. See `CONTRIBUTING.md`.

## `polaris-market`

| Code | Variant | Meaning | Returned by | Suggested user message |
| ---: | --- | --- | --- | --- |
| 1 | `NotInitialized` | Contract has not been initialized | any entrypoint before `initialize` | "Market is not set up yet." |
| 2 | `AlreadyInitialized` | `initialize` called twice | `initialize` | "Market is already set up." |
| 3 | `InvalidStrikePrice` | Strike price out of range | `initialize` | "Invalid strike price." |
| 4 | `InvalidExpiry` | Expiry not in the future | `initialize` | "Invalid expiry." |
| 5 | `InvalidGracePeriod` | Grace period out of range | `initialize` | "Invalid grace period." |
| 6 | `InvalidFeedId` | Pyth Lazer feed id invalid | `initialize` | "Invalid price feed." |
| 7 | `InvalidFeeBps` | Fee above `MAX_FEE_BPS` | `initialize` | "Fee is too high." |
| 8 | `InvalidLiquidity` | Initial liquidity invalid | `initialize` | "Invalid initial liquidity." |
| 9 | `MarketSettledOrCancelled` | Market is not in `Open` status | `split`, `merge`, `buy`, `sell`, `transfer`, `get_price`, `get_fee`, `get_pool_growth` | "This market is no longer open." |
| 10 | `MarketExpiredAwaitingSettlement` | Trading window has closed | `buy`, `sell` | "Trading has closed for this market." |
| 11 | `InvalidAmount` | Amount is zero or otherwise invalid | `split`, `merge`, `buy`, `sell`, `transfer`, `redeem` | "Enter a valid amount." |
| 12 | `InsufficientBalance` | Caller lacks the shares/collateral | `merge`, `sell`, `transfer`, `redeem` | "Insufficient balance." |
| 13 | `SlippageExceeded` | Trade output below `min_out` | `buy`, `sell` | "Price moved — slippage limit exceeded." |
| 14 | `ExpiryNotReached` | `settle` called before expiry | `settle` | "Market has not expired yet." |
| 15 | `InvalidPayload` | Malformed oracle update payload | `settle` | "Invalid price update." |
| 16 | `FeedNotFound` | Feed id not present in update | `settle` | "Price feed not found in update." |
| 17 | `StalePrice` | Price older than freshness window | `settle` | "Price update is stale." |
| 18 | `FuturePrice` | Price timestamped in the future | `settle` | "Price update timestamp is invalid." |
| 19 | `GracePeriodNotElapsed` | `cancel` before `expiry + grace` | `cancel` | "Grace period has not elapsed." |
| 20 | `AlreadyFinalized` | Market already resolved/cancelled | `settle`, `cancel` | "Market is already finalized." |
| 21 | `NotFinalized` | Market not yet resolved/cancelled | `redeem` | "Market is not finalized yet." |
| 22 | `NothingToRedeem` | Caller holds no winning shares | `redeem` | "Nothing to redeem." |
| 23 | `PoolDepthExceeded` | Trade would drain an AMM reserve | `buy`, `sell` | "Trade is too large for the current pool." |
| 24 | `ReflectorPriceUnavailable` | Reflector returned no price | `settle` | "Secondary oracle price unavailable." |
| 25 | `ReflectorPriceStale` | Reflector price too old | `settle` | "Secondary oracle price is stale." |
| 26 | `ReflectorDecimalsOutOfRange` | Reflector decimals out of range | `settle` | "Secondary oracle misconfigured." |
| 27 | `OracleDivergence` | Lazer and secondary oracle disagree | `settle` | "Oracles disagree — settlement rejected." |
| 28 | `InvalidReflectorConfig` | Reflector config invalid at init | `initialize` | "Invalid secondary oracle config." |
| 29 | `ReflectorDecimalsDriftedSinceInit` | Reflector decimals changed since init | `settle` | "Secondary oracle changed — settlement rejected." |
| 30 | `RedstonePriceUnavailable` | RedStone returned no price | `settle` | "Third oracle price unavailable." |
| 31 | `RedstonePriceStale` | RedStone price too old | `settle` | "Third oracle price is stale." |
| 32 | `RedstoneDecimalsOutOfRange` | RedStone decimals out of range | `settle` | "Third oracle misconfigured." |
| 33 | `RedstoneDecimalsDriftedSinceInit` | RedStone decimals changed since init | `settle` | "Third oracle changed — settlement rejected." |
| 34 | `InvalidRedstoneConfig` | RedStone config invalid at init | `initialize` | "Invalid third oracle config." |
| 35 | `Paused` | Admin paused trading | `buy`, `sell` | "Trading is paused." |
| 36 | `InvalidRecipient` | `transfer` target is this contract | `transfer` | "Cannot send shares to the contract." |
| 37 | `InvalidCollateral` | Collateral token reports >18 decimals | `initialize` | "Unsupported collateral token." |
| 38 | `NoPendingAdmin` | `accept_admin` with no pending proposal | `accept_admin` | "No admin change is pending." |
| 39 | `Unauthorized` | Caller is not the expected admin | `propose_admin`, `accept_admin` | "Not authorized." |
| 40 | `ReflectorPriceFutureDated` | Reflector price timestamped ahead of ledger time | `settle`, `settle_via_reflector` | "Secondary oracle price timestamp is invalid." |
| 41 | `RedstonePriceFutureDated` | RedStone price timestamped ahead of ledger time | `settle`, `settle_via_reflector` | "Third oracle price timestamp is invalid." |

## `polaris-perpetual`

| Code | Variant | Meaning | Returned by | Suggested user message |
| ---: | --- | --- | --- | --- |
| 1 | `NotInitialized` | Contract not initialized | any entrypoint before `initialize` | "Market is not set up yet." |
| 2 | `AlreadyInitialized` | `initialize` called twice | `initialize` | "Market is already set up." |
| 3 | `InvalidFeeBps` | Fee above `MAX_FEE_BPS` or an inverted curve | `initialize`, `set_fee_curve` | "Invalid fee settings." |
| 4 | `InvalidLiquidity` | Initial liquidity invalid | `initialize` | "Invalid initial liquidity." |
| 5 | `InvalidFeedId` | Feed id is the unset value 0 | `initialize` | "Invalid price feed." |
| 6 | `MarketNotOpen` | Instance was terminated | split/merge/trade entrypoints, `get_price`, `get_fee` | "This market is no longer open." |
| 7 | `InvalidAmount` | Amount is zero or negative | `split`, `merge`, `buy`, `sell`, `transfer` | "Enter a valid amount." |
| 8 | `InsufficientBalance` | Caller lacks the shares | `merge`, `sell`, `transfer` | "Insufficient balance." |
| 9 | `SlippageExceeded` | Trade output below `min_out` | `buy`, `sell` | "Price moved — slippage limit exceeded." |
| 10 | `PoolDepthExceeded` | Trade would drain an AMM reserve | `buy`, `sell` | "Trade is too large for the current pool." |
| 11 | `NotFinalized` | `redeem` before `terminate` | `redeem` | "Market is not finalized yet." |
| 12 | `NothingToRedeem` | Caller holds no shares | `redeem` | "Nothing to redeem." |
| 13 | `AlreadyFinalized` | `terminate` called twice | `terminate` | "Market is already finalized." |
| 14 | `Unauthorized` | Caller is not the admin | `terminate` | "Not authorized." |
| 15 | `PriceOracleNotConfigured` | Checkpointing on an instance with no oracle | `record_price_checkpoint` | "No price oracle configured." |
| 16 | `InvalidPayload` | Malformed oracle update payload | `record_price_checkpoint` | "Invalid price update." |
| 17 | `FeedNotFound` | Feed id not present in update | `record_price_checkpoint` | "Price feed not found in update." |
| 18 | `StalePrice` | Price older than the freshness window | `record_price_checkpoint` | "Price update is stale." |
| 19 | `FuturePrice` | Price timestamped in the future | `record_price_checkpoint` | "Price update timestamp is invalid." |
| 20 | `ReflectorPriceUnavailable` | Reflector returned no price | `record_price_checkpoint` | "Secondary oracle price unavailable." |
| 21 | `ReflectorPriceStale` | Reflector price too old | `record_price_checkpoint` | "Secondary oracle price is stale." |
| 22 | `ReflectorDecimalsOutOfRange` | Reflector decimals out of range | `initialize`, `record_price_checkpoint` | "Secondary oracle misconfigured." |
| 23 | `OracleDivergence` | Lazer and Reflector disagree | `record_price_checkpoint` | "Oracles disagree — checkpoint rejected." |
| 24 | `InvalidReflectorConfig` | Reflector config invalid at init | `initialize` | "Invalid secondary oracle config." |
| 25 | `ReflectorDecimalsDriftedSinceInit` | Reflector decimals changed since init | `record_price_checkpoint` | "Secondary oracle changed — checkpoint rejected." |
| 26 | `RedstonePriceUnavailable` | RedStone returned no price | `record_price_checkpoint` | "Third oracle price unavailable." |
| 27 | `RedstonePriceStale` | RedStone price too old | `record_price_checkpoint` | "Third oracle price is stale." |
| 28 | `RedstoneDecimalsOutOfRange` | RedStone decimals out of range | `initialize`, `record_price_checkpoint` | "Third oracle misconfigured." |
| 29 | `RedstoneDecimalsDriftedSinceInit` | RedStone decimals changed since init | `record_price_checkpoint` | "Third oracle changed — checkpoint rejected." |
| 30 | `InvalidRedstoneConfig` | RedStone config invalid at init | `initialize` | "Invalid third oracle config." |
| 31 | `Paused` | Admin paused trading | `buy`, `sell` | "Trading is paused." |
| 32 | `InvalidRecipient` | `transfer` target is this contract | `transfer` | "Cannot send shares to the contract." |
| 33 | `InvalidCollateral` | Collateral token reports >18 decimals | `initialize` | "Unsupported collateral token." |
| 34 | `FeeIncreaseNotAllowed` | `set_fee_curve` would raise a bound | `set_fee_curve` | "Fee settings may only be lowered." |
| 35 | `StaleCheckpoint` | Checkpoint payload older than the last one | `record_price_checkpoint` | "Price checkpoint is older than the latest one." |
| 36 | `ReflectorPriceFutureDated` | Reflector price timestamped ahead of ledger time | `record_price_checkpoint` | "Secondary oracle price timestamp is invalid." |
| 37 | `RedstonePriceFutureDated` | RedStone price timestamped ahead of ledger time | `record_price_checkpoint` | "Third oracle price timestamp is invalid." |

## `polaris-vault`

| Code | Variant | Meaning | Returned by | Suggested user message |
| ---: | --- | --- | --- | --- |
| 1 | `NotInitialized` | Vault not initialized | any entrypoint before `initialize` | "Vault is not set up yet." |
| 2 | `AlreadyInitialized` | `initialize` called twice | `initialize` | "Vault is already set up." |
| 3 | `InvalidAmount` | Deposit/withdraw amount invalid | `deposit`, `withdraw` | "Enter a valid amount." |
| 4 | `InsufficientShares` | Caller lacks vault shares | `withdraw` | "Insufficient shares." |
| 5 | `InsufficientBalance` | Vault lacks underlying balance | `withdraw` | "Insufficient balance." |
| 6 | `Unauthorized` | Caller is not the admin | admin-only entrypoints | "Not authorized." |
| 7 | `Paused` | Admin paused deposits/withdrawals | `deposit`, `withdraw` | "Vault is paused." |

## `polaris-smart-wallet`

| Code | Variant | Meaning | Returned by | Suggested user message |
| ---: | --- | --- | --- | --- |
| 1 | `NotInitialized` | Wallet not initialized | any entrypoint before `initialize` | "Wallet is not set up yet." |
| 2 | `AlreadyInitialized` | `initialize` called twice | `initialize` | "Wallet is already set up." |
| 3 | `Unauthorized` | Signer not authorized | `execute`, `add_signer`, `remove_signer` | "Not authorized." |
| 4 | `InvalidSignature` | Signature verification failed | `execute` | "Invalid signature." |
| 5 | `SignerNotFound` | Signer does not exist | `remove_signer` | "Signer not found." |
| 6 | `TooManySigners` | Signer set is full | `add_signer` | "Too many signers." |
| 7 | `InvalidThreshold` | Threshold out of range | `initialize`, `set_threshold` | "Invalid signature threshold." |
| 8 | `ReplayDetected` | Nonce already used | `execute` | "Transaction already submitted." |

## `polaris-smart-wallet-factory`

| Code | Variant | Meaning | Returned by | Suggested user message |
| ---: | --- | --- | --- | --- |
| 1 | `NotInitialized` | Factory not initialized | any entrypoint before `initialize` | "Factory is not set up yet." |
| 2 | `AlreadyInitialized` | `initialize` called twice | `initialize` | "Factory is already set up." |
| 3 | `Unauthorized` | Caller is not the admin | admin-only entrypoints | "Not authorized." |
| 4 | `InvalidSalt` | Deployment salt already used | `deploy` | "Wallet already exists for this salt." |
| 5 | `DeployFailed` | Underlying deploy call failed | `deploy` | "Wallet deployment failed." |
| 6 | `WalletNotFound` | No wallet registered for address | `lookup` | "Wallet not found." |
