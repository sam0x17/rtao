// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/runtime_api.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Bootstrap seed: method moved verbatim from legacy_api.rs.
// Hosted cargo xtask extract must replace this seed with authenticated output.
pub(crate) fn method(api: &str, name: &str) -> Option<LegacyMethod> {
    let (params, output, accounts, stake): (&[_], _, _, _) = match (api, name) {
        ("DelegateInfoRuntimeApi", "get_delegated") => {
            (&["Vec<u8>"], "Vec<DelegateInfo>", Accounts::One, false)
        }
        ("DelegateInfoRuntimeApi", "get_delegates") => {
            (&[], "Vec<DelegateInfo>", Accounts::None, false)
        }
        ("NeuronInfoRuntimeApi", "get_neuron_lite") => {
            (&["u16", "u16"], "NeuronInfoLite", Accounts::None, false)
        }
        ("NeuronInfoRuntimeApi", "get_neurons_lite") => {
            (&["u16"], "Vec<NeuronInfoLite>", Accounts::None, false)
        }
        ("NeuronInfoRuntimeApi", "get_neuron") => {
            (&["u16", "u16"], "NeuronInfo", Accounts::None, false)
        }
        ("NeuronInfoRuntimeApi", "get_neurons") => {
            (&["u16"], "Vec<NeuronInfo>", Accounts::None, false)
        }
        ("StakeInfoRuntimeApi", "get_stake_info_for_coldkey") => {
            (&["Vec<u8>"], "Vec<StakeInfo>", Accounts::One, true)
        }
        ("StakeInfoRuntimeApi", "get_stake_info_for_coldkeys") => (
            &["Vec<Vec<u8>>"],
            "Vec<(Vec<u8>, Vec<StakeInfo>)>",
            Accounts::Many,
            false,
        ),
        ("SubnetInfoRuntimeApi", "get_subnet_hyperparams") => (
            &["u16"],
            "Option<SubnetHyperparameters>",
            Accounts::None,
            false,
        ),
        ("SubnetInfoRuntimeApi", "get_subnet_info") => {
            (&["u16"], "Option<SubnetInfo>", Accounts::None, false)
        }
        ("SubnetInfoRuntimeApi", "get_subnet_info_v2") => {
            (&["u16"], "Option<SubnetInfoV2>", Accounts::None, false)
        }
        ("SubnetInfoRuntimeApi", "get_subnets_info" | "get_subnets_info_v2") => {
            (&[], "Vec<Option<SubnetInfo>>", Accounts::None, false)
        }
        _ => return None,
    };
    Some(LegacyMethod {
        params,
        output,
        accounts,
        stake,
    })
}
