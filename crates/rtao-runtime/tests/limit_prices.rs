//! D022 changes the public price vocabulary without changing pinned SCALE bytes.
#![allow(clippy::unwrap_used)]

use rtao_core::runtime::Runtime;
use rtao_primitives::{AccountId, AlphaBalance, NetUid, PriceRao, TaoBalance};
use rtao_runtime::{Call, RuntimeHandle, calls::subtensor_module as calls};

const HOTKEY: AccountId = AccountId([0x31; 32]);
const DESTINATION_HOTKEY: AccountId = AccountId([0x72; 32]);
const NETUID: NetUid = NetUid(0x0123);
const DESTINATION_NETUID: NetUid = NetUid(0x0456);
const AMOUNT: u64 = 0x1817_1615_1413_1211;
const AMOUNT_BYTES: [u8; 8] = [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18];
const PRICES: [u64; 4] = [0, 1, 0x0807_0605_0403_0201, u64::MAX];

fn runtime() -> RuntimeHandle {
    RuntimeHandle::new(
        Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap(),
    )
    .unwrap()
}

fn assert_wire(runtime: &RuntimeHandle, call: Call, index: u8, fields: &[&[u8]]) {
    // These are the pinned pallet/call indices and raw wire primitives, without
    // using the generated descriptors or PriceRao's encoder for the expectation.
    let mut expected = vec![7, index];
    for field in fields {
        expected.extend_from_slice(field);
    }
    assert_eq!(call.compose(runtime).unwrap().as_bytes(), expected);
}

#[test]
fn all_seven_public_limit_arguments_use_price_rao() {
    type AddStakeLimit = fn(AccountId, NetUid, TaoBalance, PriceRao, bool) -> Call;
    type RemoveStakeLimit = fn(AccountId, NetUid, AlphaBalance, PriceRao, bool) -> Call;
    type SwapStakeLimit = fn(AccountId, NetUid, NetUid, AlphaBalance, PriceRao, bool) -> Call;
    type RemoveStakeFullLimit = fn(AccountId, NetUid, Option<PriceRao>) -> Call;
    type AddStakeBurn = fn(AccountId, NetUid, TaoBalance, Option<PriceRao>) -> Call;
    type AddCollateral = fn(NetUid, AccountId, AlphaBalance, PriceRao) -> Call;
    type MoveStakeLimit =
        fn(AccountId, AccountId, NetUid, NetUid, AlphaBalance, PriceRao, bool) -> Call;

    let _: AddStakeLimit = calls::add_stake_limit;
    let _: RemoveStakeLimit = calls::remove_stake_limit;
    let _: SwapStakeLimit = calls::swap_stake_limit;
    let _: RemoveStakeFullLimit = calls::remove_stake_full_limit;
    let _: AddStakeBurn = calls::add_stake_burn;
    let _: AddCollateral = calls::add_collateral;
    let _: MoveStakeLimit = calls::move_stake_limit;
}

#[test]
fn required_price_arguments_preserve_exact_u64_wire_bytes() {
    let runtime = runtime();
    for price in PRICES {
        let price_bytes = price.to_le_bytes();
        for allow_partial in [false, true] {
            let partial = [u8::from(allow_partial)];
            assert_wire(
                &runtime,
                calls::add_stake_limit(
                    HOTKEY,
                    NETUID,
                    TaoBalance(AMOUNT),
                    PriceRao(price),
                    allow_partial,
                ),
                88,
                &[
                    &HOTKEY.0,
                    &[0x23, 0x01],
                    &AMOUNT_BYTES,
                    &price_bytes,
                    &partial,
                ],
            );
            assert_wire(
                &runtime,
                calls::remove_stake_limit(
                    HOTKEY,
                    NETUID,
                    AlphaBalance(AMOUNT),
                    PriceRao(price),
                    allow_partial,
                ),
                89,
                &[
                    &HOTKEY.0,
                    &[0x23, 0x01],
                    &AMOUNT_BYTES,
                    &price_bytes,
                    &partial,
                ],
            );
            assert_wire(
                &runtime,
                calls::swap_stake_limit(
                    HOTKEY,
                    NETUID,
                    DESTINATION_NETUID,
                    AlphaBalance(AMOUNT),
                    PriceRao(price),
                    allow_partial,
                ),
                90,
                &[
                    &HOTKEY.0,
                    &[0x23, 0x01],
                    &[0x56, 0x04],
                    &AMOUNT_BYTES,
                    &price_bytes,
                    &partial,
                ],
            );
            assert_wire(
                &runtime,
                calls::move_stake_limit(
                    HOTKEY,
                    DESTINATION_HOTKEY,
                    NETUID,
                    DESTINATION_NETUID,
                    AlphaBalance(AMOUNT),
                    PriceRao(price),
                    allow_partial,
                ),
                149,
                &[
                    &HOTKEY.0,
                    &DESTINATION_HOTKEY.0,
                    &[0x23, 0x01],
                    &[0x56, 0x04],
                    &AMOUNT_BYTES,
                    &price_bytes,
                    &partial,
                ],
            );
        }
        assert_wire(
            &runtime,
            calls::add_collateral(NETUID, HOTKEY, AlphaBalance(AMOUNT), PriceRao(price)),
            144,
            &[&[0x23, 0x01], &HOTKEY.0, &AMOUNT_BYTES, &price_bytes],
        );
    }
}

#[test]
fn optional_price_arguments_preserve_some_and_none_wire_bytes() {
    let runtime = runtime();
    for price in PRICES.into_iter().map(Some).chain([None]) {
        let mut encoded_price = Vec::new();
        match price {
            Some(value) => {
                encoded_price.push(1);
                encoded_price.extend_from_slice(&value.to_le_bytes());
            }
            None => encoded_price.push(0),
        }
        assert_wire(
            &runtime,
            calls::remove_stake_full_limit(HOTKEY, NETUID, price.map(PriceRao)),
            103,
            &[&HOTKEY.0, &[0x23, 0x01], &encoded_price],
        );
        assert_wire(
            &runtime,
            calls::add_stake_burn(HOTKEY, NETUID, TaoBalance(AMOUNT), price.map(PriceRao)),
            132,
            &[&HOTKEY.0, &[0x23, 0x01], &AMOUNT_BYTES, &encoded_price],
        );
    }
}
