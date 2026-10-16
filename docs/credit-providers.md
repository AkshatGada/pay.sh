# Merchant credit-provider policy

Configure credit providers beside accepted currencies:

```yaml
operator:
  currencies:
    usd: [USDC, USDT]
  credit_providers: []
```

An omitted `credit_providers` field means an empty list. It does not change
existing payment behavior. The policy is merchant-wide and applies before
charge, session or subscription payment handling.

`coinflow` is a recognized provider name, but its integration is not available
yet. Selecting `credit_providers: [coinflow]` fails startup rather than
advertising unsupported funding. Custom Pay middleware states also reject the
configuration before issuing or accepting payment credentials. Unknown provider
names fail deserialization.

The SDK exposes the same enum and availability check as `CreditProvider` and
`validate_credit_providers`. Its `PayKitConfig.credit_providers` field sits
beside currency configuration.

A named provider will select a reviewed integration: supported deployments,
callback entrypoints, account-resolution rules, funding authorization and refund
backing. It is not an alias for arbitrary hook JSON. Coinflow's actual adapter
and preparation contract must be supplied and reviewed before the enum can
enable it.

The raw-hook configuration described in [MPP channel versions](mpp-channel-versions.md)
remains an advanced integration/testing interface. Enabling V2 channel charge
or sessions does not by itself enable a credit provider.
