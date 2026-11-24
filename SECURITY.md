# Security policy

These contracts custody real collateral. If you find a vulnerability, please
report it privately rather than opening a public issue or PR.

## Reporting

Use this repository's **Security → Report a vulnerability** (GitHub private
vulnerability reporting) at
<https://github.com/PolarisQ-xyz/polaris-contracts/security/advisories/new>.
If that isn't available to you, contact a maintainer privately through GitHub
and ask for a private channel — do not disclose the details in a public
issue.

Include the affected contract/version or contract ID, the steps to reproduce
(or a minimal test), and the impact you believe it has. Please give us a
reasonable window to investigate and ship a fix before disclosing publicly.

## Response expectations

- **Acknowledgement:** within 72 hours of a private report.
- **Triage and severity assessment:** within 7 days; we'll tell you whether
  we've reproduced it and what we're doing about it.
- **Fix and disclosure:** critical issues affecting funds are prioritized and
  coordinated with the reporter; non-critical issues follow the normal PR
  process. We'll credit reporters who want it when the advisory is published.

This is a best-effort commitment by a small maintainer team, not a guarantee.

## Scope

In scope: the contract code under `contracts/` (market, perpetual, vault,
smart wallet, factory), the shared oracle math in `contracts/ctf-math`, and the
deployment/oracle tooling in `tools/` and `scripts/` where it affects funds or
oracle configuration. Issues with a demonstrable impact on collateral,
settlement correctness, access control, or key/origin handling are the highest
priority.

Out of scope: testnet instances and demo frontends, third-party oracle
contracts and their feeds, issues requiring a compromised admin key by
definition, and theoretical findings with no demonstrated impact. Report
those through the normal issue tracker if they're still worth tracking.

For the project's development and review process, see [CONTRIBUTING.md](./CONTRIBUTING.md).
