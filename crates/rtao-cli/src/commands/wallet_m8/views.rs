// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/helpers.py and cli/commands/wallet.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.

async fn overview_targets(
    ctx: &mut AppContext,
    all: bool,
    history: &mut PromptHistory,
) -> Result<Vec<(String, AccountId)>, CliError> {
    if !all {
        // Python captures this label before prompting for the target account.
        let name = ctx.globals.wallet_name.clone();
        let address = ctx
            .resolve_target("coldkey_ss58", None, history)?
            .ok_or_else(|| input("missing coldkey"))?;
        return Ok(vec![(name, address)]);
    }
    let rows = super::list_records(&ctx.globals.wallet_path).await?;
    let targets = rows
        .iter()
        .filter_map(|row| Some((row["coldkey"].as_str()?, row["ss58"].as_str()?)))
        .filter(|(_, address)| !address.is_empty())
        .map(|(name, address)| Ok((name.to_owned(), address.parse().map_err(failure)?)))
        .collect::<Result<Vec<_>, CliError>>()?;
    if targets.is_empty() {
        return Err(failure(format!(
            "no wallets found in {}",
            ctx.globals.wallet_path.display()
        )));
    }
    Ok(targets)
}

async fn batch_uids(
    view: &Snapshot,
    pairs: &[(NetUid, AccountId)],
) -> Result<BTreeMap<(NetUid, AccountId), u16>, CliError> {
    if pairs.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut plan = rtao::client::ReadPlan::new();
    for (netuid, hotkey) in pairs {
        plan.push(
            rtao_runtime::storage::subtensor_module::UIDS.surface(),
            vec![
                CoreValue::Int(i128::from(netuid.0)),
                CoreValue::Bytes(hotkey.0.to_vec()),
            ],
        );
    }
    let values = view.fetch(plan).await.map_err(failure)?;
    let mut result = BTreeMap::new();
    for (pair, value) in pairs.iter().copied().zip(values) {
        if matches!(value.as_core(), CoreValue::Null) {
            continue;
        }
        let uid = value
            .as_u128()
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| failure("registered UID is not native u16"))?;
        result.insert(pair, uid);
    }
    Ok(result)
}

pub(super) async fn overview(
    ctx: &mut AppContext,
    client: &Client,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    use rtao::reads::reads::{m6_locks, m6_staking};
    let targets = overview_targets(ctx, enabled(matches, "all"), history).await?;
    let filter = matches.get_one::<u16>("netuid").copied().map(NetUid);
    let view = snapshot(client, matches).await?;
    let accounts = targets
        .iter()
        .map(|(_, address)| *address)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let (valuations, balances) = tokio::try_join!(
        owner::stake_value_for_coldkeys(&view, accounts.clone()),
        owner::balances(&view, accounts)
    )
    .map_err(failure)?;
    let mut rows = Vec::new();
    let names = local_names(ctx).await?;
    let mut human_rows = Vec::new();
    let mut hidden = 0usize;
    let mut total_values = Vec::new();
    let mut accrued_values = Vec::new();
    let mut root_values = Vec::new();
    let unnamed = valuations
        .values()
        .flat_map(|v| v.positions.iter())
        .map(|p| p.hotkey)
        .filter(|h| !names.contains_key(h))
        .collect();
    let identities = owner::hotkey_identities(&view, unnamed)
        .await
        .map_err(failure)?;
    let pairs = valuations
        .values()
        .flat_map(|v| v.positions.iter())
        .filter(|p| p.is_registered && filter.is_none_or(|n| p.netuid == n))
        .map(|p| (p.netuid, p.hotkey))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let uids = batch_uids(&view, &pairs).await?;
    for (name, coldkey) in targets {
        // Duplicate wallet aliases each keep their row, as Python does.
        let valuation = valuations
            .get(&coldkey)
            .ok_or_else(|| failure("stake valuation omitted a requested account"))?;
        let netuids = valuation
            .positions
            .iter()
            .map(|p| p.netuid)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let (locks, availability, accrued) = tokio::try_join!(
            m6_locks::locks_for_coldkey(&view, coldkey),
            m6_staking::stake_availability_for_coldkey(&view, coldkey, netuids),
            owner::root_basket_owed(&view, coldkey)
        )
        .map_err(failure)?;
        let locks = locks
            .into_iter()
            .map(|row| (row.netuid, row))
            .collect::<BTreeMap<_, _>>();
        let availability = availability
            .into_iter()
            .map(|row| (row.netuid, row))
            .collect::<BTreeMap<_, _>>();
        let staked = tao(sum(
            valuation
                .positions
                .iter()
                .filter(|p| p.netuid == NetUid::ROOT)
                .map(|p| p.stake.rao),
            "root principal",
        )?);
        let yields = yield_record(&accrued, &staked);
        accrued_values.push(accrued.rao);
        root_values.push(staked.rao);
        let locked_values = availability
            .values()
            .filter(|row| row.locked.rao > 0)
            .map(|row| spot(&row.locked, valuation).map(|b| b.rao))
            .collect::<Result<Vec<_>, _>>()?;
        let locked = tao(sum(locked_values.iter().copied(), "locked value")?);
        let mut stakes = Vec::new();
        let selected = valuation
            .positions
            .iter()
            .filter(|p| filter.is_none_or(|n| p.netuid == n))
            .collect::<Vec<_>>();
        let mut subnet_values = BTreeMap::<NetUid, u64>::new();
        for position in &selected {
            let value = spot(&position.stake, valuation)?;
            total_values.push(value.rao);
            let total = subnet_values.entry(position.netuid).or_default();
            *total = total
                .checked_add(value.rao)
                .ok_or_else(|| input("subnet value exceeds native u64 rao"))?;
        }
        for position in selected {
            let value = spot(&position.stake, valuation)?;
            let uid = position
                .is_registered
                .then(|| uids.get(&(position.netuid, position.hotkey)).copied())
                .flatten();
            let mut record = json!({
                "netuid":position.netuid,"hotkey":position.hotkey,"stake":display(&position.stake),
                "stake_amount":position.stake.amount(),"stake_unit":stake_unit(position.netuid),
                "value":display(&value),"value_tao":value.amount(),"registered":position.is_registered,"uid":uid,
            });
            if let Some(avail) = availability.get(&position.netuid) {
                record["locked"] = display(&avail.locked);
                record["locked_amount"] = json!(avail.locked.amount());
                record["available"] = display(&avail.available);
                record["available_amount"] = json!(avail.available.amount());
                if avail.locked.rao > 0
                    && let Some(lock) = locks.get(&position.netuid)
                {
                    record["lock_hotkey"] = json!(lock.hotkey);
                    record["is_perpetual"] = json!(lock.is_perpetual);
                }
            }
            extend_yield(&mut record, &yields)?;
            stakes.push(record);
            if enabled(matches, "dust")
                || (value.rao >= DUST_RAO
                    && subnet_values
                        .get(&position.netuid)
                        .is_some_and(|v| *v >= DUST_RAO))
            {
                let avail = availability.get(&position.netuid);
                let note = avail
                    .filter(|a| a.locked.rao > 0)
                    .map(|a| {
                        format!(
                            "{} locked · {} free",
                            a.locked.to_python_string(),
                            a.available.to_python_string()
                        )
                    })
                    .unwrap_or_default();
                human_rows.push(vec![
                    json!(name),
                    json!(position.netuid),
                    json!(uid),
                    display(&position.stake),
                    display(&value),
                    json!(label(&position.hotkey, &names, &identities)),
                    json!(position.hotkey),
                    json!(note),
                ]);
            } else {
                hidden += 1;
            }
        }
        let free = balances.get(&coldkey).cloned().unwrap_or_else(|| tao(0));
        let mut row = json!({
            "wallet":name,"coldkey":coldkey,"free":display(&free),"free_tao":free.amount(),
            "stake_value":display(&valuation.stake_value),"stake_value_tao":valuation.stake_value.amount(),
            "stake_value_basis":BASIS,"locked_value":display(&locked),"locked_value_tao":locked.amount(),
            "locked_subnets":locked_values.len(),"positions":stakes.len(),"stakes":stakes,
        });
        extend_yield(&mut row, &yields)?;
        if !ctx.globals.json && !enabled(matches, "all") {
            let mut fields = vec![
                ("wallet".into(), json!(format!("{name} ({coldkey})"))),
                ("free".into(), display(&free)),
                (
                    "stake_value".into(),
                    json!(format!(
                        "{}  (spot, excl. slippage/fees)",
                        valuation.stake_value.to_python_string()
                    )),
                ),
            ];
            if !locked_values.is_empty() {
                fields.push((
                    "locked_value".into(),
                    json!(format!(
                        "{}  (conviction-locked; part of stake_value)",
                        locked.to_python_string()
                    )),
                ));
            }
            if accrued.rao > 0 || staked.rao > 0 {
                fields.push(("accrued_basket_yield".into(), display(&accrued)));
            }
            ctx.output.detail(None, &fields, None).map_err(failure)?;
        }
        rows.push(row);
    }
    if ctx.globals.json {
        return ctx.output.value(&rows).map_err(failure);
    }
    let mut table = TableSpec::new(
        STAKE_TITLE,
        [
            "wallet",
            "netuid",
            "uid",
            "stake",
            "value (τ)",
            "validator",
            "hotkey",
            "lock",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        human_rows,
    );
    table.right_align.extend([1, 2, 3, 4]);
    if hidden > 0 {
        table.footer = Some(format!(
            "{hidden} dust positions hidden — pass --dust to show"
        ));
    }
    ctx.output.columns(&table).map_err(failure)?;
    ctx.output
        .value(&format!(
            "total {}  (spot, excl. slippage/fees)",
            tao(sum(total_values, "overview total")?).to_python_string()
        ))
        .map_err(failure)?;
    let accrued = tao(sum(accrued_values, "combined basket yield")?);
    let staked = tao(sum(root_values, "combined root principal")?);
    if accrued.rao > 0 || staked.rao > 0 {
        let ratio = ratio::integer_division(i128::from(accrued.rao), staked.rao)
            .map(|r| format!("  ({:.2}% of root stake)", r * 100.0))
            .unwrap_or_default();
        ctx.output
            .value(&format!(
                "accrued basket yield  {}{ratio}\nauto-claim is off → rtao root claim",
                accrued.to_python_string()
            ))
            .map_err(failure)?;
    }
    Ok(())
}

pub(super) async fn registrations(
    ctx: &mut AppContext,
    client: &Client,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    use rtao::reads::reads::m6_accounts;
    let all = enabled(matches, "all");
    let filter = matches.get_one::<u16>("netuid").copied().map(NetUid);
    let listed = super::list_records(&ctx.globals.wallet_path).await?;
    let mut targets = Vec::new();
    for row in listed {
        let Some(name) = row["coldkey"].as_str() else {
            continue;
        };
        if !all && name != ctx.globals.wallet_name {
            continue;
        }
        let Some(address) = row["ss58"].as_str().filter(|a| !a.is_empty()) else {
            continue;
        };
        let coldkey = address.parse::<AccountId>().map_err(failure)?;
        let mut local = Vec::new();
        for hotkey in row["hotkeys"].as_array().into_iter().flatten() {
            if let (Some(name), Some(address)) = (
                hotkey["name"].as_str(),
                hotkey["ss58"].as_str().filter(|a| !a.is_empty()),
            ) {
                local.push((
                    name.to_owned(),
                    address.parse::<AccountId>().map_err(failure)?,
                ));
            }
        }
        targets.push((name.to_owned(), coldkey, local));
    }
    if targets.is_empty() {
        if all {
            return Err(failure(format!(
                "no wallets found in {}",
                ctx.globals.wallet_path.display()
            )));
        }
        let address = ctx
            .resolve_target("coldkey_ss58", None, history)?
            .ok_or_else(|| input("missing coldkey"))?;
        targets.push((ctx.globals.wallet_name.clone(), address, Vec::new()));
    }
    let view = snapshot(client, matches).await?;
    let mut wallets = Vec::new();
    let mut rows = Vec::new();
    let mut table_rows = Vec::new();
    let mut count = 0;
    let mut registration_count = 0;
    for (name, coldkey, local) in targets {
        let owned = m6_accounts::owned_hotkeys(&view, coldkey)
            .await
            .map_err(failure)?;
        let mut hotkeys = Vec::<(Option<String>, AccountId, bool)>::new();
        for (name, address) in local {
            if let Some(entry) = hotkeys.iter_mut().find(|entry| entry.1 == address) {
                entry.0 = Some(name);
            } else {
                hotkeys.push((Some(name), address, true));
            }
        }
        for address in owned {
            if !hotkeys.iter().any(|entry| entry.1 == address) {
                hotkeys.push((None, address, false));
            }
        }
        hotkeys.sort_by(|a, b| {
            b.2.cmp(&a.2).then_with(|| {
                natural_cmp(
                    a.0.as_deref().unwrap_or(&a.1.to_string()),
                    b.0.as_deref().unwrap_or(&b.1.to_string()),
                )
            })
        });
        wallets.push((name, coldkey, hotkeys));
    }
    let mut netuids_by_hotkey = BTreeMap::new();
    let mut pairs = Vec::new();
    for (_, _, hotkeys) in &wallets {
        for (_, hotkey, _) in hotkeys {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                netuids_by_hotkey.entry(*hotkey)
            {
                let netuids = match filter {
                    Some(netuid) => vec![netuid],
                    None => m6_accounts::netuids_for_hotkey(&view, *hotkey)
                        .await
                        .map_err(failure)?,
                };
                pairs.extend(netuids.iter().map(|netuid| (*netuid, *hotkey)));
                entry.insert(netuids);
            }
        }
    }
    let uids = batch_uids(&view, &pairs).await?;
    for (name, coldkey, hotkeys) in wallets {
        let mut records = Vec::new();
        for (label, hotkey, local) in hotkeys {
            let netuids = netuids_by_hotkey
                .get(&hotkey)
                .ok_or_else(|| failure("registration subnet cache missing hotkey"))?;
            let registrations = netuids
                .iter()
                .filter_map(|netuid| {
                    uids.get(&(*netuid, hotkey))
                        .map(|uid| json!({"netuid":netuid,"uid":uid}))
                })
                .collect::<Vec<_>>();
            count += 1;
            registration_count += registrations.len();
            if let Some(label) = &label {
                ctx.output
                    .name_address(&hotkey.to_string(), label)
                    .map_err(failure)?;
            }
            if registrations.is_empty() {
                table_rows.push(vec![
                    json!(name),
                    json!(label.as_deref().unwrap_or("—")),
                    json!("—"),
                    json!("—"),
                    json!(hotkey),
                ]);
            } else {
                for registration in &registrations {
                    table_rows.push(vec![
                        json!(name),
                        json!(label.as_deref().unwrap_or("—")),
                        registration["netuid"].clone(),
                        registration["uid"].clone(),
                        json!(hotkey),
                    ]);
                }
            }
            records.push(
                json!({"name":label,"hotkey":hotkey,"local":local,"registrations":registrations}),
            );
        }
        rows.push(json!({"wallet":name,"coldkey":coldkey,"hotkeys":records}));
    }
    let title = filter
        .map(|n| format!("wallet registrations — netuid {n}"))
        .unwrap_or_else(|| "wallet registrations".into());
    if !all {
        for row in &mut table_rows {
            row.remove(0);
        }
    }
    let columns = if all {
        vec!["wallet", "hotkey", "netuid", "uid", "hotkey_ss58"]
    } else {
        vec!["hotkey", "netuid", "uid", "hotkey_ss58"]
    };
    let mut table = TableSpec::new(
        title,
        columns.into_iter().map(str::to_owned).collect(),
        table_rows,
    );
    table.records = Some(json!(rows));
    table.right_align.extend(if all { [2, 3] } else { [1, 2] });
    table.footer = Some(format!(
        "{count} hotkey{}  ·  {registration_count} registration{}",
        if count == 1 { "" } else { "s" },
        if registration_count == 1 { "" } else { "s" }
    ));
    ctx.output.columns(&table).map_err(failure)
}
