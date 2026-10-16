# Pay CLI agent plugin

The official [Pay](https://pay.sh) plugin for AI agents, backed by a wallet on
your own machine. Discover priced APIs and pay per request (HTTP 402, x402,
MPP) with stablecoins, without API keys. Every payment is confirmed by you
before funds move.

This is a portable [Agent Plugins v1](https://agent-plugins.org/specification)
package. It contains:

- **`mcp.json`**: the local Pay MCP server, `pay mcp`, run over stdio by the
  `pay` CLI you install yourself.
- **`skills/pay`**: an [Agent Skill](https://agentskills.io) that teaches the
  agent how to pick a provider, plan spend, make the smallest useful paid call,
  and stop on a declined payment.

The package itself contains no executable code and nothing that downloads or
updates itself. If you would rather not keep a wallet locally, use the hosted
[`pay`](../pay/README.md) plugin instead; both can be installed side by side.

## Install in Hermes Agent

Install the `pay` CLI from its official distribution and set up a wallet,
outside Hermes:

```bash
brew install pay            # macOS
npm install -g @solana/pay  # any platform
pay setup
```

Then install the plugin from the Hermes plugin catalog:

```bash
hermes plugins install pay-cli
hermes plugins enable pay-cli
```

Or directly from this repository:

```bash
hermes plugins install solana-foundation/pay/agent-plugins/pay-cli --no-enable
hermes plugins enable pay-cli
```

The `pay-cli` tools load in your next session (or after `/reload-mcp`). Hermes
offers them only while `pay` is on your `PATH`.

On a headless host, `pay setup --backend file` stores an unencrypted,
owner-only keypair file; use it only if that host is a trusted wallet boundary.

## What to know before installing

- **Spends money.** The `curl` tool pays HTTP 402 challenges from your local
  wallet's stablecoin balance. The server runs with `PAY_FORCE_ELICITATION=1`,
  so Pay asks for confirmation of each payment through Hermes (including on the
  messaging gateway) instead of a local Touch ID prompt, and Hermes declines it
  when nobody can answer (cron, unattended runs). The plugin also marks the
  server `untrusted`, so Hermes asks before each write-capable call.
- **Runs a local process.** Hermes starts `pay mcp` from your `PATH` as a
  long-running stdio server for the session. The binary comes from your own
  install, not from this package.
- **Network calls.** Paid requests reach the catalog provider you choose
  through the Pay gateway, and the CLI submits each confirmed payment to the network.
- **Credentials.** The wallet key stays in the store `pay setup` configured
  (the OS keychain by default). The plugin reads no credential files.
- **Shared wallet.** Every Hermes profile and chat on this machine that uses
  the plugin spends from the same local wallet.
- **No telemetry** is sent by the plugin.

## License

MIT
