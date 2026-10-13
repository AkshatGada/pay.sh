//! Conservative fee policy for narrowly scoped, sponsored transactions.
//!
//! Callers must separately restrict instruction programs (in particular, no
//! signature-verification precompiles) and reject address lookup tables. The
//! base fee here assumes the current Solana rate of 5,000 lamports/signature.

use pay_api_core::Error;
use solana_pubkey::Pubkey;
use solana_transaction::versioned::VersionedTransaction;

pub(super) const COMPUTE_BUDGET_PROGRAM_ID: &str = "ComputeBudget111111111111111111111111111111";
const MAX_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;

/// Reject budgets we cannot price, then bound the total before signing.
pub(super) fn validate(tx: &VersionedTransaction, max_fee_lamports: u64) -> Result<(), Error> {
    if sponsored_fee_lamports(tx)? > max_fee_lamports {
        return Err(Error::InvalidPaymentCredential);
    }
    Ok(())
}

/// Upper bound, not an RPC estimate. An omitted CU limit uses the full runtime
/// maximum so builtin cost changes and instruction ordering cannot underprice
/// a sponsored transaction. Explicit limits are clamped as in the runtime.
pub(super) fn sponsored_fee_lamports(tx: &VersionedTransaction) -> Result<u64, Error> {
    let keys = tx.message.static_account_keys();
    let compute: Pubkey = COMPUTE_BUDGET_PROGRAM_ID.parse().expect("valid program id");
    let mut unit_limit = None;
    let mut unit_price = None;
    for instruction in tx.message.instructions() {
        let program = keys
            .get(usize::from(instruction.program_id_index))
            .ok_or(Error::InvalidPaymentCredential)?;
        if *program != compute {
            continue;
        }
        if !instruction.accounts.is_empty() {
            return Err(Error::InvalidPaymentCredential);
        }
        let data = instruction.data.as_slice();
        match (data.first().copied(), data.len()) {
            (Some(2), 5) if unit_limit.is_none() => {
                unit_limit = Some(u32::from_le_bytes(
                    data[1..].try_into().expect("length checked"),
                ));
            }
            (Some(3), 9) if unit_price.is_none() => {
                unit_price = Some(u64::from_le_bytes(
                    data[1..].try_into().expect("length checked"),
                ));
            }
            _ => return Err(Error::InvalidPaymentCredential),
        }
    }
    let unit_limit = unit_limit
        .unwrap_or(MAX_COMPUTE_UNIT_LIMIT)
        .min(MAX_COMPUTE_UNIT_LIMIT);
    // Widen before multiplication and round up exactly as the runtime does.
    let priority_fee =
        (u128::from(unit_limit) * u128::from(unit_price.unwrap_or(0))).div_ceil(1_000_000);
    let base_fee = u128::from(tx.message.header().num_required_signatures) * 5_000;
    u64::try_from(base_fee + priority_fee).map_err(|_| Error::InvalidPaymentCredential)
}
