# #29 — Multi-outcome / categorical markets

Status: **decision note only.** No N-way contract is included here; this
closes the tracking issue with a chosen direction and the reasoning behind
it.

## 1. The gap

Every market in `polaris-market` / `polaris-perpetual` is a binary threshold
on one strike price: yes/no against a single number. A real prediction-market
product eventually wants "which of N outcomes wins?" (e.g. an election with
five candidates), which a two-reserve constant-product AMM cannot represent.

## 2. Decision

**No N-way market now.** Binary markets remain the product's scope for the
current stage. When multi-outcome demand becomes concrete, build the small
**market-group wrapper** (issue option 2): present N mutually-exclusive binary
markets under one UI/entrypoint, composing the already-proven
`polaris-market` contract without changing its AMM math.

Do **not** start the LMSR / generalized-CPMM rewrite (option 1) without both
explicit demand and a dedicated audit. It is a new invariant, a new set of
adversarial tests and a new deployed contract — not an incremental change to
this one.

## 3. Open questions answered

### Does the fee-curve / slippage-floor logic generalize cleanly?

No — not mechanically. `effective_fee_bps` uses `total_supply` as a single
volume proxy, and the CPMM out/pricing formulas assume exactly two reserves
that must sum to one outcome. An N-way pool needs a new depth measure, a new
"shares sum to one winner" invariant and a fresh audit even if it reuses the
same base/min bps constants. The wrapper avoids this question entirely:
each binary leg keeps the existing math, and the wrapper itself adds no new
AMM.

### Is there near-term demand?

None has been established. Binary markets cover the current product, so the
issue should be closed with a recorded decision rather than kept open with an
unscoped implementation. File a follow-up with a concrete market/customer
requirement when that changes.

## 4. Why the wrapper is the first step

- Reuses settlement, collateral accounting, the fee curve, pause, and the
  existing solvency/property tests — no shared math to fork or re-audit.
- The wrapper can be deployed and retired independently of the market
  contract it composes.
- Honest limitation: N independent binary markets do **not** enforce "exactly
  one winner" on-chain. A wrapper that only groups them is a presentation /
  integration layer, not a coordinator. Guaranteeing mutual exclusivity
  requires a new contract that locks collateral once and pays against the
  winning leg — that is effectively option 1's audit burden and must be
  scoped as its own issue.

## 5. Reopening trigger

Reopen (or file a follow-up referencing this note) when there is a concrete
market with N > 2 outcomes and a named operator for it. At that point choose
between (a) the wrapper plus a thin mutual-exclusivity coordinator, or (b) an
N-way AMM, based on whether on-chain exclusivity is actually required.
