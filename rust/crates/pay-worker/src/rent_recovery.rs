//! Reclaim-only recovery for atomic V2 charges without a session snapshot.
//! This module must never advance a channel through settlement or distribution.

use pay_api_core::rpc::RpcClient;
use pay_kit::core::{payment_channels, payment_channels_v2};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use crate::{channel::STATUS_DISTRIBUTED, error::JobError};

pub const PROGRAM_ID: Pubkey =
    Pubkey::from_str_const(payment_channels_v2::PAYMENT_CHANNELS_PROGRAM_ID);

pub fn enabled(network: &str) -> bool {
    matches!(network, "devnet" | "localnet")
}

/// V2 retains the fixed base layout: rent payer occupies bytes 216..248.
const RENT_PAYER_OFFSET: usize = 216;

/// Scope discovery to this operator, not every merchant's terminal channels.
/// The RPC wrapper supports one memcmp; status is checked locally and on fetch.
pub async fn discover(
    rpc: &RpcClient,
    rpc_url: &str,
    rent_payer: &Pubkey,
) -> Result<Vec<Pubkey>, JobError> {
    let accounts = rpc
        .get_program_accounts_filtered(
            rpc_url,
            &PROGRAM_ID.to_string(),
            payment_channels_v2::CHANNEL_ACCOUNT_SIZE,
            RENT_PAYER_OFFSET,
            rent_payer.as_ref(),
        )
        .await?;
    let mut addresses = Vec::new();
    for account in accounts {
        // Discovery is only a hint: owner is verified by a fresh account fetch
        // under the reclaim lease before any instruction is built.
        if let Ok(decoded) = deployment().decode_channel(&account.data)
            && decoded.channel.status == STATUS_DISTRIBUTED
            && decoded.channel.rent_payer.to_bytes() == rent_payer.to_bytes()
        {
            addresses.push(
                account
                    .pubkey
                    .parse()
                    .map_err(|_| JobError::InvalidAddress(account.pubkey))?,
            );
        }
    }
    Ok(addresses)
}

fn deployment() -> payment_channels_v2::Deployment {
    payment_channels_v2::Deployment {
        program_id: PROGRAM_ID,
        version: 2,
    }
}

/// Absence is an idempotent success; any live account must pass every check.
pub fn plan(
    address: &Pubkey,
    account: Option<(&Pubkey, &[u8])>,
    rent_payer: &Pubkey,
    current_slot: u64,
) -> Result<Option<Instruction>, JobError> {
    let Some((owner, data)) = account else {
        return Ok(None);
    };
    let decoded = deployment()
        .decode_account(owner, data)
        .map_err(|error| JobError::Config(format!("V2 rent recovery: {error}")))?;
    if decoded.guard != 0
        || decoded.channel.status != STATUS_DISTRIBUTED
        || decoded.channel.rent_payer.to_bytes() != rent_payer.to_bytes()
    {
        return Err(JobError::Config(
            "V2 rent recovery eligibility mismatch".into(),
        ));
    }
    let end = decoded
        .channel
        .open_slot
        .checked_add(payment_channels::OPEN_SLOT_WINDOW)
        .ok_or_else(|| JobError::Config("V2 reclaim window overflow".into()))?;
    if current_slot <= end {
        return Ok(None);
    }
    // Reclaim invokes no hooks and returns rent only to the immutable rent payer.
    Ok(Some(payment_channels::build_reclaim_instruction(
        address,
        rent_payer,
        &PROGRAM_ID,
    )))
}

pub async fn fetch_plan(
    rpc: &RpcClient,
    rpc_url: &str,
    address: &Pubkey,
    rent_payer: &Pubkey,
    current_slot: u64,
) -> Result<Option<Instruction>, JobError> {
    let accounts = rpc
        .get_multiple_accounts_with_owner(rpc_url, &[address.to_string()])
        .await?;
    let Some(account) = accounts.into_iter().next().flatten() else {
        return plan(address, None, rent_payer, current_slot);
    };
    let owner = account
        .owner
        .parse()
        .map_err(|_| JobError::InvalidAddress(account.owner))?;
    plan(
        address,
        Some((&owner, &account.data)),
        rent_payer,
        current_slot,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rent_payer: &Pubkey) -> Vec<u8> {
        let hooks = payment_channels_v2::ChannelHooks::default();
        let mut data = vec![0; payment_channels_v2::CHANNEL_ACCOUNT_SIZE];
        data[0] = 1;
        data[1] = 2;
        data[3] = STATUS_DISTRIBUTED;
        data[RENT_PAYER_OFFSET..RENT_PAYER_OFFSET + 32].copy_from_slice(rent_payer.as_ref());
        data[248..256].copy_from_slice(&100u64.to_le_bytes());
        data[265..297].copy_from_slice(&hooks.config_hash().unwrap());
        data[297..].copy_from_slice(&hooks.encode_fixed().unwrap());
        data
    }

    #[test]
    fn distributed_without_session_can_only_reclaim_after_window() {
        let payer = Pubkey::new_unique();
        let address = Pubkey::new_unique();
        let data = fixture(&payer);
        for slot in [0, 100, 1599, 1600] {
            assert!(
                plan(&address, Some((&PROGRAM_ID, &data)), &payer, slot)
                    .unwrap()
                    .is_none()
            );
        }
        let ix = plan(&address, Some((&PROGRAM_ID, &data)), &payer, 1601)
            .unwrap()
            .unwrap();
        assert_eq!(
            ix,
            payment_channels::build_reclaim_instruction(&address, &payer, &PROGRAM_ID)
        );
        assert!(plan(&address, None, &payer, 1601).unwrap().is_none());
    }

    #[test]
    fn rejects_nonterminal_wrong_owner_payer_layout_version_and_guard() {
        let payer = Pubkey::new_unique();
        let address = Pubkey::new_unique();
        let data = fixture(&payer);
        let other = Pubkey::new_unique();
        assert!(plan(&address, Some((&other, &data)), &payer, 1601).is_err());
        assert!(plan(&address, Some((&PROGRAM_ID, &data)), &other, 1601).is_err());
        for (offset, value) in [(3, 0), (3, 1), (3, 2), (3, 4), (0, 0), (1, 1), (256, 1)] {
            let mut bad = data.clone();
            bad[offset] = value;
            assert!(plan(&address, Some((&PROGRAM_ID, &bad)), &payer, 1601).is_err());
        }
        assert!(plan(&address, Some((&PROGRAM_ID, &data[..256])), &payer, 1601).is_err());
        let mut overflow = data;
        overflow[248..256].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(plan(&address, Some((&PROGRAM_ID, &overflow)), &payer, u64::MAX).is_err());
    }

    #[test]
    fn network_gate_is_explicit() {
        assert!(enabled("devnet"));
        assert!(enabled("localnet"));
        for network in ["mainnet", "mainnet-beta", "sandbox", "testnet", ""] {
            assert!(!enabled(network));
        }
    }

    #[tokio::test]
    async fn discovery_scopes_rpc_to_rent_payer_and_excludes_open_and_sealed() {
        use axum::{Json, Router, routing::post};
        use base64::Engine;
        use serde_json::{Value, json};
        use std::{sync::Arc, time::Duration};
        use tokio::sync::Mutex;

        let payer = Pubkey::new_unique();
        let address = Pubkey::new_unique();
        let mut entries = Vec::new();
        for status in [0, 1, 2, STATUS_DISTRIBUTED] {
            let mut data = fixture(&payer);
            data[3] = status;
            entries.push(json!({
                "pubkey": address.to_string(),
                "account": { "data": [base64::engine::general_purpose::STANDARD.encode(data), "base64"] }
            }));
        }
        let request = Arc::new(Mutex::new(None));
        let captured = request.clone();
        let app = Router::new().route(
            "/",
            post(move |Json(body): Json<Value>| {
                let captured = captured.clone();
                let entries = entries.clone();
                async move {
                    *captured.lock().await = Some(body);
                    Json(json!({"jsonrpc": "2.0", "id": 1, "result": entries}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let rpc = RpcClient::new(Duration::from_secs(5)).unwrap();
        let result = discover(&rpc, &url, &payer).await;
        server.abort();
        let _ = server.await;
        assert_eq!(result.unwrap(), vec![address]);
        let request = request.lock().await.take().unwrap();
        assert_eq!(request["method"], "getProgramAccounts");
        assert_eq!(request["params"][0], PROGRAM_ID.to_string());
        assert_eq!(
            request["params"][1]["filters"],
            json!([
                {"dataSize": payment_channels_v2::CHANNEL_ACCOUNT_SIZE},
                {"memcmp": {"offset": RENT_PAYER_OFFSET, "bytes": payer.to_string()}}
            ])
        );
    }
}
