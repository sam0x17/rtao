// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/transactions.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_evm.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Legacy EIP-155 transactions and EIP-191 hotkey association proofs.

use crate::{
    EvmAccount, EvmError, Result,
    rpc::{EvmRpc, TransactionReceipt, wei_to_balance},
};
use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_primitives::{Address, B256, Signature, TxKind, U256, keccak256};
use alloy_signer::SignerSync;
use rtao_primitives::{AccountId, DynBalance};
use serde_json::{Value, json};
use std::time::Duration;

/// Resolved transaction fields shown before signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmTxPreview {
    /// Address that will sign this transaction.
    pub sender: Address,
    /// Recipient, or `None` for contract deployment.
    pub to: Option<Address>,
    /// EVM value with eighteen decimal places.
    pub value_wei: U256,
    /// Call data, or deployment init code.
    pub data: Vec<u8>,
    /// Resolved gas limit.
    pub gas: u64,
    /// Legacy gas price in wei per gas unit.
    pub gas_price_wei: U256,
    /// Sender's resolved latest nonce.
    pub nonce: u64,
    /// EIP-155 replay-protection chain identifier.
    pub chain_id: u64,
}

impl EvmTxPreview {
    /// The value in checked native TAO rao, truncating sub-rao dust.
    pub fn value(&self) -> Result<DynBalance> {
        wei_to_balance(self.value_wei)
    }

    /// The maximum fee in checked native TAO rao, truncating sub-rao dust.
    pub fn max_fee(&self) -> Result<DynBalance> {
        let wei = self
            .gas_price_wei
            .checked_mul(U256::from(self.gas))
            .ok_or_else(|| EvmError::invalid("EVM maximum fee exceeds 256 bits"))?;
        wei_to_balance(wei)
    }

    /// Python-shaped public transaction preview with integer JSON amounts.
    pub fn to_json(&self) -> Result<Value> {
        Ok(json!({
            "from": self.sender.to_checksum(None),
            "to": self.to.map(|address| address.to_checksum(None)).unwrap_or_else(|| "(contract creation)".into()),
            "value_tao": decimal_balance(&self.value()?),
            "value_wei": decimal_json(self.value_wei)?,
            "data": format!("0x{}", hex::encode(&self.data)),
            "gas": self.gas,
            "gas_price_wei": decimal_json(self.gas_price_wei)?,
            "max_fee_tao": decimal_balance(&self.max_fee()?),
            "nonce": self.nonce,
            "chain_id": self.chain_id,
        }))
    }

    fn legacy(&self) -> Result<TxLegacy> {
        // Validate both public values even for manually constructed previews.
        self.value()?;
        self.max_fee()?;
        if self.data.len() > 8 * 1024 * 1024 {
            return Err(EvmError::invalid("EVM transaction data exceeds 8 MiB"));
        }
        let gas_price = u128::try_from(self.gas_price_wei)
            .map_err(|_| EvmError::invalid("legacy EVM gas price exceeds u128"))?;
        Ok(TxLegacy {
            chain_id: Some(self.chain_id),
            nonce: self.nonce,
            gas_price,
            gas_limit: self.gas,
            to: self.to.map(TxKind::Call).unwrap_or(TxKind::Create),
            value: self.value_wei,
            input: self.data.clone().into(),
        })
    }
}

fn decimal_json(value: U256) -> Result<Value> {
    serde_json::from_str(&value.to_string()).map_err(Into::into)
}

fn decimal_balance(value: &DynBalance) -> String {
    value
        .to_decimal_string()
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// Estimate a transaction before signing, then resolve price, nonce and chain ID.
///
/// A failed gas estimate stops preparation immediately. No key is accessed and
/// no transaction is submitted by this function.
pub async fn prepare_transaction(
    rpc: &EvmRpc,
    sender: Address,
    to: Option<Address>,
    value_wei: U256,
    data: &[u8],
) -> Result<EvmTxPreview> {
    wei_to_balance(value_wei)?;
    if data.len() > 8 * 1024 * 1024 {
        return Err(EvmError::invalid("EVM transaction data exceeds 8 MiB"));
    }
    let mut estimate = json!({
        "from": format!("{sender:#x}"), "value": format!("{value_wei:#x}"),
        "data": format!("0x{}", hex::encode(data)),
    });
    if let Some(to) = to {
        estimate["to"] = json!(format!("{to:#x}"));
    }
    let gas = rpc.estimate_gas(&estimate).await?;
    let gas_price_wei = rpc.gas_price().await?;
    let nonce = rpc.get_nonce(sender).await?;
    let chain_id = rpc.chain_id().await?;
    let preview = EvmTxPreview {
        sender,
        to,
        value_wei,
        data: data.to_owned(),
        gas,
        gas_price_wei,
        nonce,
        chain_id,
    };
    preview.legacy()?;
    Ok(preview)
}

/// Signed legacy transaction bytes and their precomputed recovery hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedEvmTransaction {
    /// Exact RLP bytes transmitted to `eth_sendRawTransaction`.
    pub raw: Vec<u8>,
    /// Keccak-256 of `raw`, known before any submission.
    pub hash: B256,
}

/// Sign a resolved transaction with EIP-155 replay protection.
///
/// A different sender or signer-configured chain ID refuses before signing.
pub fn sign_transaction(
    account: &EvmAccount,
    preview: &EvmTxPreview,
) -> Result<SignedEvmTransaction> {
    if account.address() != preview.sender {
        return Err(EvmError::invalid(
            "EVM preview sender does not match the signing account",
        ));
    }
    if account
        .chain_id()
        .is_some_and(|chain| chain != preview.chain_id)
    {
        return Err(EvmError::invalid(
            "EVM preview chain ID does not match the signing account",
        ));
    }
    let transaction = preview.legacy()?;
    let signature = account
        .sign_hash_sync(&transaction.signature_hash())
        .map_err(|_| EvmError::invalid("EVM transaction signing failed"))?;
    let signed = transaction.into_signed(signature);
    let mut raw = Vec::with_capacity(signed.rlp_encoded_length());
    signed.rlp_encode(&mut raw);
    Ok(SignedEvmTransaction {
        hash: keccak256(&raw),
        raw,
    })
}

/// Result of one transaction submission, optionally including its receipt.
#[derive(Clone, Debug, PartialEq)]
pub struct EvmTxResult {
    /// Exact attempted transaction hash, including when no wait was requested.
    pub tx_hash: B256,
    /// Receipt observed during this operation, if requested.
    pub receipt: Option<TransactionReceipt>,
}

impl EvmTxResult {
    /// Python-shaped submission facts, without uninterpreted receipt fields.
    pub fn to_json(&self) -> Value {
        let mut value = json!({"tx_hash": format!("{:#x}", self.tx_hash)});
        if let Some(receipt) = &self.receipt {
            value["block_number"] = json!(receipt.block_number);
            value["gas_used"] = json!(receipt.gas_used);
            value["success"] = json!(receipt.success);
            if let Some(address) = receipt.contract_address {
                value["contract_address"] = json!(address.to_checksum(None));
            }
        }
        value
    }
}

/// Sign and submit once, then optionally observe a receipt for up to 120 seconds.
///
/// Every error after submission begins retains the exact transaction hash in
/// [`EvmError::UnknownFate`]. Callers can reconcile it without signing or replaying.
pub async fn send_transaction(
    rpc: &EvmRpc,
    account: &EvmAccount,
    preview: &EvmTxPreview,
    wait: bool,
) -> Result<EvmTxResult> {
    send_transaction_with_timeout(
        rpc,
        account,
        preview,
        wait.then_some(Duration::from_secs(120)),
    )
    .await
}

/// Submit with an explicit receipt deadline, or return immediately after acceptance.
pub async fn send_transaction_with_timeout(
    rpc: &EvmRpc,
    account: &EvmAccount,
    preview: &EvmTxPreview,
    receipt_timeout: Option<Duration>,
) -> Result<EvmTxResult> {
    let signed = sign_transaction(account, preview)?;
    let unresolved = |cause: EvmError| {
        if cause.unknown_fate_hash() == Some(signed.hash) {
            cause
        } else {
            EvmError::UnknownFate {
                hash: signed.hash,
                cause: Box::new(cause),
            }
        }
    };
    let hash = rpc
        .send_raw_transaction(&signed.raw)
        .await
        .map_err(unresolved)?;
    let receipt = match receipt_timeout {
        Some(timeout) => Some(
            rpc.wait_for_receipt(hash, timeout)
                .await
                .map_err(unresolved)?,
        ),
        None => None,
    };
    Ok(EvmTxResult {
        tx_hash: hash,
        receipt,
    })
}

/// The exact sixty-four-byte ownership message, before EIP-191 framing.
pub fn association_message(hotkey: AccountId, block_number: u64) -> [u8; 64] {
    let mut message = [0; 64];
    message[..32].copy_from_slice(&hotkey.0);
    message[32..].copy_from_slice(keccak256(block_number.to_le_bytes()).as_slice());
    message
}

/// EIP-191 personal-message signature for `associate_evm_key`.
///
/// The message is `hotkey_pubkey || keccak256(SCALE(u64 block_number))`.
/// The signature contains `r || s || v`, with `v` in Ethereum's 27/28 convention.
pub fn association_proof(
    account: &EvmAccount,
    hotkey_ss58: &str,
    block_number: u64,
) -> Result<(String, u64)> {
    let hotkey: AccountId = hotkey_ss58
        .parse()
        .map_err(|error: rtao_core::CoreError| EvmError::invalid(error.to_string()))?;
    let signature = account
        .sign_message_sync(&association_message(hotkey, block_number))
        .map_err(|_| EvmError::invalid("EVM association signing failed"))?;
    Ok((
        format!("0x{}", hex::encode(signature.as_bytes())),
        block_number,
    ))
}

/// Recover the EVM owner from an association proof without network access.
pub fn recover_association_owner(
    hotkey: AccountId,
    block_number: u64,
    signature: &[u8],
) -> Result<Address> {
    let signature = Signature::from_raw(signature)
        .map_err(|_| EvmError::invalid("invalid EVM association signature"))?;
    signature
        .recover_address_from_msg(association_message(hotkey, block_number))
        .map_err(|_| EvmError::invalid("invalid EVM association signature"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_signer::Signer;

    type TestError = Box<dyn std::error::Error + Send + Sync>;
    type ScriptedServer = tokio::task::JoinHandle<std::result::Result<Vec<Value>, TestError>>;

    fn account() -> Result<EvmAccount> {
        EvmAccount::from_slice(&[0x46; 32]).map_err(|_| EvmError::invalid("fixture key"))
    }

    fn preview(account: &EvmAccount) -> EvmTxPreview {
        EvmTxPreview {
            sender: account.address(),
            to: Some(Address::from([0x35; 20])),
            value_wei: U256::from(1_000_000_000_000_000_000u64),
            data: vec![],
            gas: 21_000,
            gas_price_wei: U256::from(20_000_000_000u64),
            nonce: 9,
            chain_id: 1,
        }
    }

    #[test]
    fn official_eip155_signed_transaction_vector() -> Result<()> {
        // https://eips.ethereum.org/EIPS/eip-155#example
        let account = account()?;
        let signed = sign_transaction(&account, &preview(&account))?;
        assert_eq!(
            hex::encode(&signed.raw),
            concat!(
                "f86c098504a817c800825208943535353535353535353535353535353535353535",
                "880de0b6b3a76400008025",
                "a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276",
                "a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83"
            )
        );
        assert_eq!(signed.hash, keccak256(&signed.raw));
        let json = preview(&account).to_json()?;
        assert_eq!(json["value_wei"], json!(1_000_000_000_000_000_000u64));
        assert_eq!(json["value_tao"], "1");
        assert_eq!(json["max_fee_tao"], "0.00042");
        Ok(())
    }

    #[test]
    fn signing_rechecks_sender_chain_native_value_and_fee() -> Result<()> {
        let mut account = account()?;
        let mut tx = preview(&account);
        tx.sender = Address::ZERO;
        assert!(sign_transaction(&account, &tx).is_err());
        tx.sender = account.address();
        account.set_chain_id(Some(964));
        assert!(sign_transaction(&account, &tx).is_err());
        account.set_chain_id(None);
        tx.value_wei = (U256::from(u64::MAX) + U256::from(1)) * U256::from(1_000_000_000u64);
        assert!(sign_transaction(&account, &tx).is_err());
        tx.value_wei = U256::ZERO;
        tx.gas_price_wei = U256::MAX;
        assert!(sign_transaction(&account, &tx).is_err());
        Ok(())
    }

    #[test]
    // pyt: tests/unit/test_evm.py::TestAssociationProof::test_block_hash_matches_scale_u64_le
    fn association_uses_scale_u64_then_sixty_four_byte_eip191_domain() -> Result<()> {
        let account = account()?;
        let hotkey = AccountId([0xab; 32]);
        for block in [0, 42, u64::MAX] {
            let message = association_message(hotkey, block);
            assert_eq!(&message[..32], &[0xab; 32]);
            assert_eq!(&message[32..], keccak256(block.to_le_bytes()).as_slice());
            let (signature, returned) = association_proof(&account, &hotkey.to_string(), block)?;
            assert_eq!(returned, block);
            let bytes =
                hex::decode(&signature[2..]).map_err(|_| EvmError::invalid("fixture signature"))?;
            assert_eq!(bytes.len(), 65);
            assert!(matches!(bytes[64], 27 | 28));
            assert_eq!(
                recover_association_owner(hotkey, block, &bytes)?,
                account.address()
            );
            assert_ne!(
                recover_association_owner(hotkey, block.wrapping_add(1), &bytes)?,
                account.address()
            );
            assert_ne!(
                recover_association_owner(AccountId([0xac; 32]), block, &bytes)?,
                account.address()
            );
        }
        assert!(recover_association_owner(hotkey, 42, &[0; 64]).is_err());
        Ok(())
    }

    async fn scripted_rpc(
        responses: Vec<Value>,
    ) -> std::result::Result<(EvmRpc, ScriptedServer), TestError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let rpc = EvmRpc::new(format!("http://{}", listener.local_addr()?))?;
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept()).await??;
                let mut input = Vec::new();
                let (body_start, length) = loop {
                    let mut chunk = [0; 1024];
                    let read =
                        tokio::time::timeout(Duration::from_secs(5), socket.read(&mut chunk))
                            .await??;
                    if read == 0 || input.len() > 64 * 1024 {
                        return Err("fixture request ended early or exceeded bound".into());
                    }
                    input.extend_from_slice(&chunk[..read]);
                    if let Some(end) = input.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&input[..end])?;
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then_some(value.trim().parse::<usize>())
                            })
                            .ok_or("missing fixture content length")??;
                        break (end + 4, length);
                    }
                };
                while input.len() < body_start + length {
                    let mut chunk = [0; 1024];
                    let read =
                        tokio::time::timeout(Duration::from_secs(5), socket.read(&mut chunk))
                            .await??;
                    if read == 0 || input.len() > 64 * 1024 {
                        return Err("fixture request body ended early or exceeded bound".into());
                    }
                    input.extend_from_slice(&chunk[..read]);
                }
                let request: Value =
                    serde_json::from_slice(&input[body_start..body_start + length])?;
                let mut envelope = json!({"jsonrpc":"2.0", "id":request["id"]});
                let key = if response.get("error").is_some() {
                    "error"
                } else {
                    "result"
                };
                envelope[key] = if key == "error" {
                    response["error"].clone()
                } else {
                    response
                };
                let body = serde_json::to_vec(&envelope)?;
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
                socket.write_all(&body).await?;
                requests.push(request);
            }
            Ok(requests)
        });
        Ok((rpc, server))
    }

    #[tokio::test]
    async fn prepare_uses_exact_rpc_order_and_stops_after_failed_estimate()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let account = account()?;
        let (rpc, server) = scripted_rpc(vec![
            json!("0x5208"),
            json!("0x4a817c800"),
            json!("0x9"),
            json!("0x1"),
        ])
        .await?;
        let tx = prepare_transaction(
            &rpc,
            account.address(),
            Some(Address::from([0x35; 20])),
            U256::from(1_000_000_000_000_000_000u64),
            &[],
        )
        .await?;
        assert_eq!(tx, preview(&account));
        let requests = server.await??;
        assert_eq!(
            requests
                .iter()
                .map(|value| value["method"].as_str())
                .collect::<Vec<_>>(),
            vec![
                Some("eth_estimateGas"),
                Some("eth_gasPrice"),
                Some("eth_getTransactionCount"),
                Some("eth_chainId")
            ]
        );
        assert_eq!(requests[0]["params"][0]["value"], "0xde0b6b3a7640000");
        assert_eq!(
            requests[0]["params"][0]["to"],
            format!("0x{}", "35".repeat(20))
        );
        let (rpc, server) = scripted_rpc(vec![
            json!({"error":{"code":-32000,"message":"insufficient funds"}}),
        ])
        .await?;
        assert!(
            prepare_transaction(&rpc, account.address(), None, U256::ZERO, &[0x60])
                .await
                .is_err()
        );
        assert_eq!(server.await??.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn send_preserves_recovery_hash_on_endpoint_error_without_retransmitting()
    -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let account = account()?;
        let preview = preview(&account);
        let signed = sign_transaction(&account, &preview)?;
        let (rpc, server) = scripted_rpc(vec![
            json!({"error":{"code":-32000,"message":"already known"}}),
        ])
        .await?;
        let error = send_transaction(&rpc, &account, &preview, false)
            .await
            .err()
            .ok_or("expected endpoint error")?;
        assert_eq!(error.unknown_fate_hash(), Some(signed.hash));
        let requests = server.await??;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["method"], "eth_sendRawTransaction");
        assert_eq!(
            requests[0]["params"][0],
            format!("0x{}", hex::encode(&signed.raw))
        );

        let (rpc, server) = scripted_rpc(vec![json!(format!("{:#x}", signed.hash))]).await?;
        let result = send_transaction(&rpc, &account, &preview, false).await?;
        assert_eq!(result.tx_hash, signed.hash);
        assert!(result.receipt.is_none());
        assert_eq!(
            result.to_json(),
            json!({"tx_hash":format!("{:#x}", signed.hash)})
        );
        assert_eq!(server.await??.len(), 1);
        Ok(())
    }
}
