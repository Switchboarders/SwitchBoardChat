# Security policy

SwitchBoardChat carries confidential conversations between people in distress and helpline
respondents. Please report security problems privately so they can be fixed before they are known.

## Reporting a vulnerability

- Use GitHub's **private vulnerability reporting**: the "Security" tab of this repository, then
  "Report a vulnerability". Do not open a public issue, pull request or discussion.
- Include what you found, how to reproduce it, and what an attacker could do with it.
- Never include real conversations, phone numbers or credentials, yours or anyone else's.

You should get an answer within 7 days. Once a fix is released the advisory is published, with
credit to you if you want it.

## Supported versions

Only the latest commit of the `main` branch is supported. There are no maintained release branches:
update to the latest `main` to get security fixes.

## Scope

In scope: this repository (the bridge, its HTTP endpoints `/voip_api`, `/sbchat_api` and `/web/*`,
its Matrix behaviour, the Docker image, the install script and the sample configurations).

Out of scope: Synapse, Matrix Authentication Service, voip.ms, Element and other upstream projects
(report to them directly), and deployments that ignore the hardening notes in
[docs/security.md](docs/security.md).

## Rules for contributors

- Never log message bodies, phone numbers, tokens, passwords or voip.ms query strings (Quebec Law 25).
- Never commit secrets: `.env` is ignored; use `.env-example` with placeholders only.
- New dependencies must pass `cargo deny check` (see `deny.toml`).
