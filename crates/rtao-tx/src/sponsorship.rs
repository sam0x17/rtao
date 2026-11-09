// Derived from RaoFoundation/subtensor sdk/python/bittensor/registration_sponsorship.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Explicit authorization for recipient registration added by the executor.
//!
//! A first payment to a hashed or post-quantum account registers it in the
//! same batch, permanently reserving a deposit from the sender, separate from
//! the payment and fees (D-148). The executor never prompts: a caller gives
//! a deposit allowance ([`ExecuteOptions::max_registration_deposit`]) or a
//! consent callback ([`ExecuteOptions::on_registration_sponsorship`]) that
//! approves exactly this quote; without either, or when either declines, the
//! submission is refused before a key is unlocked.
//!
//! [`ExecuteOptions::max_registration_deposit`]: crate::ExecuteOptions::max_registration_deposit
//! [`ExecuteOptions::on_registration_sponsorship`]: crate::ExecuteOptions::on_registration_sponsorship

use std::{fmt, sync::Arc};

use futures::future::BoxFuture;
use rtao_primitives::{AccountId, Balance, Tao};
use serde_json::{Map, Value};

use crate::IntentError;

/// Python `RegistrationSponsorship`: the permanent reserve in the composed
/// transaction, separate from its fee.
///
/// `payer_account` permanently reserves `deposit` to register
/// `recipient_account`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationSponsorship {
    /// The account being registered.
    pub recipient_account: AccountId,
    /// The account whose free balance the deposit is reserved from (the
    /// dispatch origin).
    pub payer_account: AccountId,
    /// The permanent reserve.
    pub deposit: Balance<Tao>,
}

/// A consent callback: return `true` to authorize exactly this quote.
pub type SponsorshipCallback =
    Arc<dyn Fn(RegistrationSponsorship) -> BoxFuture<'static, bool> + Send + Sync>;

/// Python's `on_registration_sponsorship`: a caller-supplied consent callback.
#[derive(Clone)]
pub struct SponsorshipConsent(pub SponsorshipCallback);

impl SponsorshipConsent {
    /// Wrap a consent callback.
    pub fn new<F, Fut>(callback: F) -> Self
    where
        F: Fn(RegistrationSponsorship) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = bool> + Send + 'static,
    {
        Self(Arc::new(move |quote| Box::pin(callback(quote))))
    }
}

impl fmt::Debug for SponsorshipConsent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SponsorshipConsent(..)")
    }
}

/// Python `RegistrationSponsorshipRequired`: automatic registration was not
/// explicitly authorized before signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SponsorshipRequired {
    /// The quote that was not authorized.
    pub quote: RegistrationSponsorship,
    /// Why (exceeds the allowance, declined, or no consent given).
    pub reason: String,
}

impl fmt::Display for SponsorshipRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: registering {} permanently reserves {} TAO from {}, in addition to the payment and transaction fees; authorize with max_registration_deposit or on_registration_sponsorship",
            self.reason,
            self.quote.recipient_account,
            self.quote.deposit.to_decimal_string(),
            self.quote.payer_account,
        )
    }
}

impl std::error::Error for SponsorshipRequired {}

/// Python's `registration_sponsorship_quote`: the automatic sponsorship in
/// the actual composed call's metadata, or `None` when it registers nothing.
#[doc(alias = "bittensor.registration_sponsorship.registration_sponsorship_quote")]
pub fn registration_sponsorship_quote(
    extras: &Map<String, Value>,
    payer_account: AccountId,
) -> Result<Option<RegistrationSponsorship>, IntentError> {
    let reserve = extras
        .get("hashed_registration_max_deposit_rao")
        .or_else(|| extras.get("hashed_registration_deposit_rao"));
    let reserve = match reserve {
        None => 0,
        Some(value) => value.as_u64().ok_or_else(|| {
            IntentError::validation("invalid automatic registration deposit quote")
        })?,
    };
    if reserve == 0 {
        return Ok(None);
    }
    let recipient = extras
        .get("hashed_registration")
        .and_then(Value::as_str)
        .filter(|recipient| !recipient.is_empty())
        .and_then(|recipient| recipient.parse::<AccountId>().ok())
        .ok_or_else(|| {
            IntentError::validation("automatic registration deposit is missing its recipient")
        })?;
    Ok(Some(RegistrationSponsorship {
        recipient_account: recipient,
        payer_account,
        deposit: Balance::from_rao(reserve),
    }))
}

/// Python's `authorize_registration_sponsorship`: authorize the composed
/// call's reserve before a submission key is unlocked.
///
/// With an allowance, a larger deposit is refused; with a callback, it must
/// return `true` for this quote; when both are given, both must approve (a
/// callback cannot override an insufficient allowance). With neither, a call
/// that reserves a deposit is refused.
#[doc(alias = "bittensor.registration_sponsorship.authorize_registration_sponsorship")]
pub async fn authorize_registration_sponsorship(
    extras: &Map<String, Value>,
    payer_account: AccountId,
    max_registration_deposit: Option<Balance<Tao>>,
    on_registration_sponsorship: Option<&SponsorshipConsent>,
) -> Result<(), crate::TxError> {
    let Some(quote) = registration_sponsorship_quote(extras, payer_account)? else {
        return Ok(());
    };
    let refuse = |quote: RegistrationSponsorship, reason: &str| {
        crate::TxError::RegistrationSponsorship(Box::new(SponsorshipRequired {
            quote,
            reason: reason.to_owned(),
        }))
    };
    if let Some(maximum) = max_registration_deposit
        && quote.deposit.rao() > maximum.rao()
    {
        return Err(refuse(
            quote,
            "registration deposit exceeds the allowed maximum",
        ));
    }
    if let Some(consent) = on_registration_sponsorship {
        if (consent.0)(quote.clone()).await {
            return Ok(());
        }
        return Err(refuse(quote, "registration sponsorship was declined"));
    }
    if max_registration_deposit.is_none() {
        return Err(refuse(quote, "registration sponsorship requires consent"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use serde_json::json;

    fn extras(deposit: u64) -> Map<String, Value> {
        json!({
            "hashed_registration": AccountId([1; 32]).to_string(),
            "hashed_registration_deposit_rao": deposit,
            "hashed_registration_max_deposit_rao": deposit,
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn quotes_come_from_the_composed_metadata_only() {
        let payer = AccountId([2; 32]);
        assert_eq!(
            registration_sponsorship_quote(&Map::new(), payer).unwrap(),
            None
        );
        let quote = registration_sponsorship_quote(&extras(5), payer)
            .unwrap()
            .unwrap();
        assert_eq!(quote.recipient_account, AccountId([1; 32]));
        assert_eq!(quote.payer_account, payer);
        assert_eq!(quote.deposit, Balance::from_rao(5));
        // A guard for an already registered recipient reserves nothing.
        let checked =
            json!({"hashed_registration_guards": [], "hashed_registration_max_deposit_rao": 0});
        assert_eq!(
            registration_sponsorship_quote(checked.as_object().unwrap(), payer).unwrap(),
            None
        );
        let missing = json!({"hashed_registration_max_deposit_rao": 3});
        assert!(registration_sponsorship_quote(missing.as_object().unwrap(), payer).is_err());
        let invalid = json!({"hashed_registration_max_deposit_rao": -3});
        assert!(registration_sponsorship_quote(invalid.as_object().unwrap(), payer).is_err());
    }

    #[tokio::test]
    async fn consent_requires_an_allowance_or_an_exact_true() {
        let payer = AccountId([2; 32]);
        let extras = extras(5);
        let refused = |result: Result<(), crate::TxError>| match result {
            Err(crate::TxError::RegistrationSponsorship(error)) => error.reason,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            refused(authorize_registration_sponsorship(&extras, payer, None, None).await),
            "registration sponsorship requires consent"
        );
        authorize_registration_sponsorship(&extras, payer, Some(Balance::from_rao(5)), None)
            .await
            .unwrap();
        assert_eq!(
            refused(
                authorize_registration_sponsorship(
                    &extras,
                    payer,
                    Some(Balance::from_rao(4)),
                    None
                )
                .await
            ),
            "registration deposit exceeds the allowed maximum"
        );
        let yes = SponsorshipConsent::new(|_| async { true });
        let no = SponsorshipConsent::new(|_| async { false });
        authorize_registration_sponsorship(&extras, payer, None, Some(&yes))
            .await
            .unwrap();
        assert_eq!(
            refused(authorize_registration_sponsorship(&extras, payer, None, Some(&no)).await),
            "registration sponsorship was declined"
        );
        // A callback cannot override an insufficient allowance.
        assert_eq!(
            refused(
                authorize_registration_sponsorship(
                    &extras,
                    payer,
                    Some(Balance::from_rao(4)),
                    Some(&yes)
                )
                .await
            ),
            "registration deposit exceeds the allowed maximum"
        );
        // Nothing to authorize without a reserve, even with no consent.
        authorize_registration_sponsorship(&Map::new(), payer, None, None)
            .await
            .unwrap();
        let message = crate::TxError::RegistrationSponsorship(Box::new(SponsorshipRequired {
            quote: registration_sponsorship_quote(&extras, payer)
                .unwrap()
                .unwrap(),
            reason: "registration sponsorship requires consent".into(),
        }))
        .to_string();
        assert!(
            message.contains("permanently reserves 0.000000005 TAO"),
            "{message}"
        );
    }
}
