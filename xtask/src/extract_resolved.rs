//! Static resolution for Python tables whose values are not plain literals.

use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
use parity_scale_codec::DecodeAll;
use rustpython_parser::{
    Parse,
    ast::{self, Ranged},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

#[path = "extract_catalog.rs"]
mod catalog;

#[path = "extract_chain.rs"]
mod chain;

#[path = "extract_env_vars.rs"]
mod env_vars;

#[path = "extract_read_params.rs"]
mod read_params;

#[path = "extract_read_hits.rs"]
mod read_hits;

#[path = "extract_pool_rpc.rs"]
mod pool_rpc;
#[path = "extract_specs.rs"]
mod specs;

#[path = "extract_py_tables.rs"]
mod py_tables;

#[path = "extract_descriptions.rs"]
mod descriptions;

#[path = "extract_hyperparams.rs"]
mod hyperparams;

#[path = "extract_owner_map.rs"]
mod owner_map;

use super::{
    Literal, MAX_ALLOCATION_UNITS, MAX_CONTAINER_ITEMS, MAX_DEPTH, MAX_SOURCE_BYTES, binary,
    constant, equal_key, git_blob, insert_entry, line, spend, unique_hashable,
};

const SETTINGS: &str = "sdk/python/bittensor/settings.py";
const RPC: &str = "sdk/python/bittensor/_transport/rpc.py";
const SETTINGS_NAMES: &[&str] = &[
    "SS58_FORMAT",
    "TAO_SYMBOL",
    "ALPHA_SYMBOL",
    "RAO_PER_TAO",
    "U16_MAX",
    "BLOCKTIME",
    "DEFAULT_ERA_PERIOD",
    "MEV_SHIELD_ERA_PERIOD",
    "GLOBAL_MAX_SUBNET_COUNT",
    "NETWORKS",
    "DEFAULT_NETWORK",
    "FINNEY_GENESIS_HASH",
    "VAULT_GUIDE_URL",
    "FALLBACK_ENDPOINTS",
    "ARCHIVE_ENDPOINTS",
    "DOCS_URL",
    "EXPLORER_EXTRINSIC_URLS",
    "EXPLORER_ACCOUNT_URLS",
    "EXPLORER_BLOCK_URLS",
    "EXPLORER_SUBNET_URLS",
];
const RPC_NAMES: &[&str] = &[
    "_STATE_DISCARDED_NEEDLE",
    "_POLICY_RPC_ERROR_CODES",
    "_POLICY_RPC_METADATA_KEYS",
    "_POLICY_RPC_ERROR_MESSAGES",
    "_HANDSHAKE_TIMEOUT",
    "_NON_IDEMPOTENT_METHODS",
    "_SUBSCRIPTION_QUEUE_MAXSIZE",
    "_CLOSED_IDS_REMEMBERED",
    "_MAX_EARLY_BUFFERS",
    "RpcSession.__init__.fallback_urls",
    "RpcSession.__init__.retry_forever",
    "RpcSession.__init__.max_retries",
    "RpcSession.__init__.response_timeout",
    "RpcSession.__init__.connect_factory",
];
const NAMED_TABLES: &[(&str, &[&str])] = &[
    ("sdk/python/bittensor/_generated/errors.py", &["ERRORS"]),
    (
        "sdk/python/bittensor/_transport/utils/receipt.py",
        &["_NESTED_RESULT_EVENTS"],
    ),
    (
        "sdk/python/bittensor/_transport/runtime.py",
        &[
            "_V15_VERSION_HEX",
            "_V15_MISSING_NEEDLE",
            "_DISK_MAGIC",
            "_DISK_FORMAT_VERSION",
            "_DISK_CACHE_MAX_FILES",
        ],
    ),
    (
        "sdk/python/bittensor/error_map.py",
        &[
            "NAME_TO_CODE",
            "CUSTOM_TRANSACTION_ERRORS",
            "INVALID_TRANSACTION_ERRORS",
            "DISPATCH_ERRORS",
        ],
    ),
    (
        "sdk/python/bittensor/result.py",
        &[
            "_CUSTOM_ERROR_RE",
            "_WATCH_STATUS_RE",
            "_WATCH_STATUS_NAMES",
            "REMEDIATION",
            "EXPLANATIONS",
            "_SUBSTRING_FALLBACK",
            "_NAME_HELP_OVERRIDES",
            "_HELP_OVERRIDES",
        ],
    ),
    (
        "sdk/python/bittensor/executor.py",
        &[
            "_TRANSIENT_SUBSTRINGS",
            "_MAX_SHIELDED_CIPHERTEXT_BYTES",
            "_SHIELDED_POLL_ATTEMPTS_PER_BLOCK",
            "_SHIELDED_FINALITY_RPC_RETRIES",
        ],
    ),
    ("sdk/python/bittensor/multisig.py", &["_FEE_ALLOWANCE_RAO"]),
    (
        "sdk/python/bittensor/hyperparams.py",
        &[
            "RATIO_TYPE_DENOMINATORS",
            "KINDS",
            "FIXED128_ONE",
            "_FRACTION_DENOMINATOR",
            "SHORT_MAX",
            "HYPERPARAMS",
            "STORAGE_ITEMS",
            "_IDENT_KINDS",
            "_IDENT_CODEC",
            "_KIND_CODEC",
        ],
    ),
    (
        "sdk/python/bittensor/intents/hyperparameters.py",
        &["OWNER_HYPERPARAMETERS", "_ALPHA_PAIR"],
    ),
    ("sdk/python/bittensor/intents/proxy.py", &["PROXY_TYPES"]),
    (
        "sdk/python/bittensor/intents/_money.py",
        &["_FALLBACK_AMOUNT_UNITS", "_IDENT_UNITS"],
    ),
    (
        "sdk/python/bittensor/fee_filters.py",
        &["COLDKEY_PAYS_FEE_CALLS", "COLDKEY_FEE_WARNING"],
    ),
    ("sdk/python/bittensor/evm/networks.py", &["EVM_NETWORKS"]),
    (
        "sdk/python/bittensor/config.py",
        &[
            "DEFAULT_CONFIG_PATH",
            "DEFAULT_PROXIES_PATH",
            "DEFAULT_ADDRESSES_PATH",
            "DEFAULT_MULTISIGS_PATH",
            "DEFAULT_MULTISIG_CACHE_PATH",
            "DEFAULT_SUBNET_NAMES_PATH",
            "DEFAULT_TOKEN_SYMBOLS_PATH",
            "SUBNET_NAMES_TTL_SECONDS",
            "TOKEN_SYMBOLS_TTL_SECONDS",
            "SETTABLE",
        ],
    ),
    (
        "sdk/python/bittensor/basket_index_data.py",
        &[
            "EPOCH_BLOCK",
            "SAMPLE_STEP",
            "GENERATED_BLOCK",
            "INDEX",
            "TR_INDEX",
            "BASELINES",
        ],
    ),
    (
        "sdk/python/codegen/check.py",
        &["COVERED_PALLETS", "RAW_ONLY"],
    ),
    (
        "sdk/python/tests/harness/samples.py",
        &[
            "ALICE",
            "ALICE_HOT",
            "BOB",
            "BOB_HOT",
            "INTENT_SAMPLES",
            "READ_SAMPLES",
        ],
    ),
    (
        "sdk/python/tests/harness/fake_substrate.py",
        &[
            "DEFAULT_STORAGE",
            "DEFAULT_CONSTANTS",
            "DEFAULT_RUNTIME",
            "GENESIS_HASH",
        ],
    ),
];

#[derive(Clone)]
enum Binding {
    Value(Literal),
    Module(String),
    Symbol(String),
    Namespace(BTreeMap<String, Self>),
    Constructor(Vec<(String, Option<Literal>)>),
    Refused(String),
}

type Bindings = BTreeMap<String, Binding>;

impl Binding {
    fn units(&self, depth: usize) -> Result<usize> {
        ensure!(depth < MAX_DEPTH, "resolved binding exceeds depth limit");
        let units = match self {
            Self::Value(value) => value.allocation_units(depth)?,
            Self::Module(name) | Self::Symbol(name) | Self::Refused(name) => {
                64usize.saturating_add(name.len())
            }
            Self::Namespace(values) => {
                values.iter().try_fold(64usize, |units, (name, value)| {
                    Ok::<_, anyhow::Error>(
                        units
                            .saturating_add(name.len())
                            .saturating_add(value.units(depth + 1)?),
                    )
                })?
            }
            Self::Constructor(fields) => {
                fields.iter().try_fold(64usize, |units, (name, value)| {
                    Ok::<_, anyhow::Error>(
                        units.saturating_add(name.len()).saturating_add(
                            value
                                .as_ref()
                                .map(|value| value.allocation_units(depth + 1))
                                .transpose()?
                                .unwrap_or(0),
                        ),
                    )
                })?
            }
        };
        ensure!(
            units <= MAX_ALLOCATION_UNITS,
            "resolved binding exceeds allocation limit"
        );
        Ok(units)
    }
}

fn clone_binding(binding: &Binding, budget: &Cell<usize>) -> Result<Binding> {
    spend(budget, binding.units(0)?)?;
    Ok(binding.clone())
}

struct Module {
    bindings: Bindings,
    locations: BTreeMap<String, (usize, usize)>,
    aliases: BTreeMap<String, String>,
    derived: BTreeMap<String, Value>,
    escaped: BTreeMap<String, BTreeSet<String>>,
}

/// Each input is authenticated against the imported git tree before parsing.
struct Resolver<'a> {
    root: &'a Path,
    tree: BTreeMap<String, String>,
    modules: BTreeMap<String, Module>,
    loading: BTreeSet<String>,
    budget: Cell<usize>,
    surface: Option<DescriptorSurface>,
    #[cfg(test)]
    sources: BTreeMap<String, String>,
}

type DescriptorSurface = (
    BTreeMap<(String, String), String>,
    BTreeSet<(String, String)>,
);

fn effects(expr: &ast::Expr, bindings: &Bindings) -> Result<BTreeSet<String>> {
    let mut changed = BTreeSet::new();
    super::effects::walk(expr, 0, &mut |expr| match expr {
        ast::Expr::NamedExpr(assign) => {
            if let Some(name) = expression_path(&assign.target) {
                changed.insert(name);
            }
        }
        ast::Expr::Call(call) => {
            let pure = call_path(&call.func).is_some_and(|name| {
                (if name == "dict.fromkeys" {
                    !bindings.contains_key("dict")
                } else {
                    !bindings.contains_key(&name)
                }) && matches!(
                    name.as_str(),
                    "list"
                        | "dict"
                        | "tuple"
                        | "set"
                        | "frozenset"
                        | "bytes"
                        | "str"
                        | "int"
                        | "float"
                        | "bool"
                        | "object"
                        | "sorted"
                        | "range"
                        | "len"
                        | "min"
                        | "max"
                        | "abs"
                        | "register"
                        | "dict.fromkeys"
                )
            });
            let declared = matches!(&*call.func,ast::Expr::Name(name) if matches!(bindings.get(name.id.as_str()),Some(Binding::Constructor(_)|Binding::Symbol(_))));
            if !pure && !declared {
                if let ast::Expr::Attribute(method) = &*call.func {
                    if !matches!(
                        method.attr.as_str(),
                        "items"
                            | "keys"
                            | "values"
                            | "get"
                            | "join"
                            | "format"
                            | "replace"
                            | "lower"
                            | "upper"
                            | "strip"
                    ) && let Some(name) = expression_path(&method.value)
                        .and_then(|path| path.split('.').next().map(str::to_owned))
                        && (!bindings.contains_key(&name)
                            || matches!(bindings.get(&name), Some(Binding::Value(_))))
                    {
                        changed.insert(name);
                    }
                    let root = expression_path(&method.value)
                        .and_then(|name| name.split('.').next().map(str::to_owned));
                    let literal_receiver = matches!(&*method.value,ast::Expr::Constant(value) if matches!(&value.value,ast::Constant::Str(_)))
                        || matches!(
                            &*method.value,
                            ast::Expr::List(_)
                                | ast::Expr::Dict(_)
                                | ast::Expr::Tuple(_)
                                | ast::Expr::Set(_)
                        );
                    let safe_receiver = (literal_receiver
                        || root.as_ref().is_some_and(|root| {
                            matches!(
                                bindings.get(root),
                                Some(Binding::Value(
                                    Literal::Dict(_)
                                        | Literal::List(_)
                                        | Literal::Str(_)
                                        | Literal::Set(_)
                                ))
                            )
                        }))
                        && matches!(
                            method.attr.as_str(),
                            "items"
                                | "keys"
                                | "values"
                                | "get"
                                | "join"
                                | "format"
                                | "replace"
                                | "lower"
                                | "upper"
                                | "strip"
                        );
                    if !safe_receiver {
                        for argument in call
                            .args
                            .iter()
                            .chain(call.keywords.iter().map(|keyword| &keyword.value))
                        {
                            if let Ok(names) = super::effects::names(argument) {
                                changed.extend(names.into_iter().filter(|name| {
                                    matches!(
                                        bindings.get(name),
                                        Some(Binding::Value(
                                            Literal::Dict(_) | Literal::List(_) | Literal::Set(_)
                                        ))
                                    )
                                }));
                            }
                        }
                    }
                } else if call.args.is_empty() && call.keywords.is_empty() {
                    changed.extend(bindings.iter().filter_map(|(name, binding)| {
                        matches!(binding, Binding::Value(_)).then_some(name.clone())
                    }));
                } else {
                    for argument in call
                        .args
                        .iter()
                        .chain(call.keywords.iter().map(|keyword| &keyword.value))
                    {
                        if let Ok(names) = super::effects::names(argument) {
                            changed.extend(names.into_iter().filter(|name| {
                                !bindings.contains_key(name)
                                    || matches!(bindings.get(name), Some(Binding::Value(_)))
                            }));
                        }
                    }
                }
            }
        }
        _ => {}
    })?;
    Ok(changed)
}

fn dependencies(expr: &ast::Expr, bindings: &Bindings) -> Result<BTreeSet<String>> {
    let mut names = super::effects::names(expr)?;
    let mut captures = Vec::new();
    super::effects::walk(expr, 0, &mut |expr| {
        if let ast::Expr::Lambda(lambda) = expr {
            captures.push((*lambda.body).clone());
        }
    })?;
    for capture in captures {
        names.extend(super::effects::names(&capture)?);
    }
    Ok(names
        .into_iter()
        .filter(|name| {
            matches!(
                bindings.get(name),
                Some(Binding::Value(
                    Literal::Dict(_) | Literal::List(_) | Literal::Set(_)
                ))
            )
        })
        .collect())
}

impl<'a> Resolver<'a> {
    fn new(root: &'a Path) -> Result<Self> {
        let mut tree: BTreeMap<String, String> =
            fs::read_to_string(root.join("vendor/upstream/tree.txt"))?
                .lines()
                .map(|entry| {
                    let (metadata, path) = entry
                        .split_once('\t')
                        .context("malformed upstream tree row")?;
                    let blob = metadata
                        .split_whitespace()
                        .nth(2)
                        .context("missing blob in upstream tree row")?;
                    Ok((path.to_owned(), blob.to_owned()))
                })
                .collect::<Result<_>>()?;
        // import-upstream retains this bounded extra input outside the watched
        // Python tree. Its artifact SHA binds the bytes to the parity pin.
        let extra = "pallets/subtensor/src/lib.rs";
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let artifact = pins["artifacts"]
            .get(extra)
            .context("pallet source artifact pin missing")?;
        let destination = format!("vendor/upstream/src/{extra}");
        ensure!(
            artifact.get("to").and_then(toml_edit::Item::as_str) == Some(destination.as_str()),
            "pallet artifact destination differs from source input"
        );
        let bytes = fs::read(root.join(destination))?;
        ensure!(
            bytes.len() <= MAX_SOURCE_BYTES,
            "pallet source exceeds extraction byte limit"
        );
        ensure!(
            artifact.get("sha256").and_then(toml_edit::Item::as_str)
                == Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
            "pallet source differs from pinned artifact SHA256"
        );
        tree.insert(extra.into(), git_blob(&bytes));
        Ok(Self {
            root,
            tree,
            modules: BTreeMap::new(),
            loading: BTreeSet::new(),
            budget: Cell::new(MAX_ALLOCATION_UNITS),
            surface: None,
            #[cfg(test)]
            sources: BTreeMap::new(),
        })
    }

    fn source(&self, path: &str) -> Result<String> {
        #[cfg(test)]
        if let Some(source) = self.sources.get(path) {
            ensure!(
                source.len() <= MAX_SOURCE_BYTES,
                "{path}: test source exceeds byte limit"
            );
            return Ok(source.clone());
        }
        let expected = self
            .tree
            .get(path)
            .with_context(|| format!("{path}: input absent from imported upstream tree"))?;
        let bytes = fs::read(self.root.join("vendor/upstream/src").join(path))
            .with_context(|| format!("read vendored extraction input {path}"))?;
        ensure!(
            bytes.len() <= MAX_SOURCE_BYTES,
            "{path}: Python source exceeds extraction byte limit"
        );
        ensure!(
            git_blob(&bytes) == *expected,
            "vendored input {path} is modified: git blob does not match imported tree"
        );
        String::from_utf8(bytes).with_context(|| format!("{path}: Python source is not UTF-8"))
    }

    fn module(&mut self, path: &str) -> Result<()> {
        if self.modules.contains_key(path) {
            return Ok(());
        }
        ensure!(
            self.loading.len() < MAX_DEPTH,
            "module resolution exceeds depth limit"
        );
        ensure!(
            self.loading.insert(path.into()),
            "{path}: circular Python imports prevent static name resolution"
        );
        let result = self.parse_module(path);
        self.loading.remove(path);
        self.modules.insert(path.into(), result?);
        Ok(())
    }

    fn parse_module(&mut self, path: &str) -> Result<Module> {
        let source = self.source(path)?;
        let suite =
            ast::Suite::parse(&source, path).map_err(|error| anyhow::anyhow!("{path}: {error}"))?;
        ensure!(
            suite.len() <= MAX_CONTAINER_ITEMS,
            "{path}: module exceeds statement limit"
        );
        let mut module = Module {
            bindings: Bindings::new(),
            locations: BTreeMap::new(),
            aliases: BTreeMap::new(),
            derived: BTreeMap::new(),
            escaped: BTreeMap::new(),
        };
        for stmt in &suite {
            match stmt {
                ast::Stmt::Import(import) => {
                    for alias in &import.names {
                        let local = alias.asname.as_ref().map_or_else(
                            || alias.name.split('.').next().unwrap_or(""),
                            |name| name.as_str(),
                        );
                        module
                            .bindings
                            .insert(local.into(), Binding::Module(alias.name.to_string()));
                    }
                }
                ast::Stmt::ImportFrom(import) => {
                    for alias in &import.names {
                        let local = alias.asname.as_ref().unwrap_or(&alias.name).to_string();
                        ensure!(
                            alias.name.as_str() != "*",
                            "{path}:{}: wildcard imports are outside static resolution",
                            line(&source, stmt.range().start().to_usize())
                        );
                        let imported = self
                            .imported_binding(path, import, alias.name.as_str())
                            .unwrap_or_else(|error| Binding::Refused(format!("{error:#}")));
                        module.bindings.insert(local, imported);
                    }
                }
                ast::Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        self.assign(&mut module, target, &assign.value, stmt, &source, path)?;
                    }
                    if assign.targets.len() > 1
                        && let Some(ast::Expr::Name(first)) = assign.targets.first()
                        && matches!(
                            module.bindings.get(first.id.as_str()),
                            Some(Binding::Value(
                                Literal::Dict(_) | Literal::List(_) | Literal::Set(_)
                            ))
                        )
                    {
                        for target in &assign.targets[1..] {
                            if let ast::Expr::Name(name) = target {
                                module
                                    .aliases
                                    .insert(name.id.to_string(), first.id.to_string());
                            }
                        }
                    }
                }
                ast::Stmt::AnnAssign(assign) => {
                    if let Some(value) = &assign.value {
                        self.assign(&mut module, &assign.target, value, stmt, &source, path)?;
                    }
                }
                ast::Stmt::ClassDef(class) => {
                    for expression in class.decorator_list.iter().chain(&class.bases) {
                        self.apply_effects(&mut module, expression, None)?;
                    }
                    let binding = self
                        .class_binding(class, &module.bindings, path)
                        .unwrap_or_else(|error| Binding::Refused(format!("{error:#}")));
                    module.bindings.insert(class.name.to_string(), binding);
                    self.collect_defaults(
                        &class.body,
                        &class.name,
                        &module.bindings.clone(),
                        &mut module,
                        &source,
                        path,
                    )?;
                }
                ast::Stmt::FunctionDef(function) => {
                    for expression in &function.decorator_list {
                        self.apply_effects(&mut module, expression, None)?;
                    }
                    self.function_defaults(
                        function.name.as_str(),
                        &function.args,
                        stmt,
                        &module.bindings.clone(),
                        &mut module,
                        &source,
                        path,
                    )?;
                    module.bindings.insert(
                        function.name.to_string(),
                        Binding::Refused("function values require a hand port".into()),
                    );
                }
                ast::Stmt::AsyncFunctionDef(function) => {
                    for expression in &function.decorator_list {
                        self.apply_effects(&mut module, expression, None)?;
                    }
                    self.function_defaults(
                        function.name.as_str(),
                        &function.args,
                        stmt,
                        &module.bindings.clone(),
                        &mut module,
                        &source,
                        path,
                    )?;
                    module.bindings.insert(
                        function.name.to_string(),
                        Binding::Refused("function values require a hand port".into()),
                    );
                }
                ast::Stmt::AugAssign(assign) => {
                    Self::invalidate_target(
                        &mut module,
                        &assign.target,
                        "augmented mutation requires a hand port",
                    );
                }
                ast::Stmt::Delete(delete) => {
                    for target in &delete.targets {
                        Self::invalidate_target(
                            &mut module,
                            target,
                            "deleted table requires a hand port",
                        );
                    }
                }
                ast::Stmt::If(block) => {
                    Self::invalidate_block(&mut module, &block.body);
                    Self::invalidate_block(&mut module, &block.orelse);
                }
                ast::Stmt::For(block) => {
                    Self::invalidate_block(&mut module, &block.body);
                    Self::invalidate_block(&mut module, &block.orelse);
                }
                ast::Stmt::While(block) => {
                    Self::invalidate_block(&mut module, &block.body);
                    Self::invalidate_block(&mut module, &block.orelse);
                }
                ast::Stmt::Expr(expr) => {
                    self.apply_effects(&mut module, &expr.value, None)?;
                }
                _ => Self::invalidate_block(&mut module, std::slice::from_ref(stmt)),
            }
        }
        Ok(module)
    }

    fn imported_binding(
        &mut self,
        path: &str,
        import: &ast::StmtImportFrom,
        name: &str,
    ) -> Result<Binding> {
        let module_name = import.module.as_ref().map_or("", |name| name.as_str());
        let level = import.level.map_or(0, |level| level.to_usize());
        if level == 0
            && matches!(
                module_name,
                "os" | "pathlib" | "dataclasses" | "re" | "enum"
            )
        {
            return Ok(Binding::Symbol(format!("{module_name}.{name}")));
        }
        let base = if level == 0 {
            PathBuf::from("sdk/python").join(module_name.replace('.', "/"))
        } else {
            let mut base = Path::new(path)
                .parent()
                .context("Python module has no parent")?
                .to_path_buf();
            for _ in 1..level {
                base.pop();
            }
            base.join(module_name.replace('.', "/"))
        };
        if matches!(
            base.to_str(),
            Some(
                "sdk/python/bittensor/_generated/storage"
                    | "sdk/python/bittensor/_generated/runtime_apis"
            )
        ) {
            return Ok(Binding::Symbol(format!("{}.{}", base.display(), name)));
        }
        let nested = base.join(name).with_extension("py");
        let nested_path = nested.to_str().context("import path is not UTF-8")?;
        if self.tree.contains_key(nested_path) {
            return Ok(Binding::Module(nested_path.into()));
        }
        let module_path = base.with_extension("py");
        let module_path = module_path.to_str().context("import path is not UTF-8")?;
        self.module(module_path)?;
        self.modules[module_path]
            .bindings
            .get(name)
            .map(|binding| clone_binding(binding, &self.budget))
            .transpose()?
            .with_context(|| format!("{path}: imported name {name} absent from {module_path}"))
    }

    fn assign(
        &mut self,
        module: &mut Module,
        target: &ast::Expr,
        expr: &ast::Expr,
        stmt: &ast::Stmt,
        source: &str,
        path: &str,
    ) -> Result<()> {
        let span = (
            line(source, stmt.range().start().to_usize()),
            line(source, stmt.range().end().to_usize()),
        );
        self.apply_effects(module, expr, None)?;
        let evaluated = self
            .eval_binding(expr, &module.bindings, path, 0)
            .unwrap_or_else(|error| Binding::Refused(format!("{path}:{}: {error:#}", span.0)));
        match target {
            ast::Expr::Name(name) => {
                let dependencies = dependencies(expr, &module.bindings)?;
                module.escaped.insert(name.id.to_string(), dependencies);
                module.aliases.remove(name.id.as_str());
                if let ast::Expr::Name(original) = expr
                    && matches!(
                        evaluated,
                        Binding::Value(Literal::Dict(_) | Literal::List(_) | Literal::Set(_))
                    )
                {
                    let original = module
                        .aliases
                        .get(original.id.as_str())
                        .cloned()
                        .unwrap_or_else(|| original.id.to_string());
                    module.aliases.insert(name.id.to_string(), original);
                }
                module.bindings.insert(name.id.to_string(), evaluated);
                module.locations.insert(name.id.to_string(), span);
            }
            ast::Expr::Subscript(subscript) => {
                let ast::Expr::Name(name) = &*subscript.value else {
                    bail!(
                        "{path}:{}: nested table mutation requires a hand port",
                        span.0
                    );
                };
                if module.aliases.contains_key(name.id.as_str())
                    || module
                        .aliases
                        .values()
                        .any(|alias| alias == name.id.as_str())
                    || module.escaped.iter().any(|(dependent, names)| {
                        dependent != name.id.as_str() && names.contains(name.id.as_str())
                    })
                {
                    Self::invalidate_target(
                        module,
                        &subscript.value,
                        "mutation of an aliased table requires a hand port",
                    );
                    return Ok(());
                }
                let key = self.eval(&subscript.slice, &module.bindings, path, 0)?;
                ensure!(
                    key.hashable(),
                    "{path}:{}: mutation key is not hashable",
                    span.0
                );
                let Binding::Value(value) = evaluated else {
                    Self::invalidate_target(
                        module,
                        &subscript.value,
                        "mutation value cannot be statically resolved",
                    );
                    return Ok(());
                };
                match module.bindings.get_mut(name.id.as_str()) {
                    Some(Binding::Value(Literal::Dict(entries))) => {
                        spend(
                            &self.budget,
                            key.allocation_units(0)?
                                .saturating_add(value.allocation_units(0)?),
                        )?;
                        insert_entry(entries, key, value);
                        ensure!(
                            entries.len() <= MAX_CONTAINER_ITEMS,
                            "{path}:{}: mutated dictionary exceeds item limit",
                            span.0
                        );
                    }
                    _ => Self::invalidate_target(
                        module,
                        &subscript.value,
                        "mutation target is not an accepted dictionary",
                    ),
                }
                if let Some(binding) = module.bindings.get(name.id.as_str()) {
                    binding.units(0)?;
                }
                if let Some(location) = module.locations.get_mut(name.id.as_str()) {
                    location.1 = span.1;
                }
            }
            _ => Self::invalidate_target(module, target, "unpacking requires a hand port"),
        }
        Ok(())
    }

    fn invalidate_target(module: &mut Module, target: &ast::Expr, reason: &str) {
        match target {
            ast::Expr::Name(name) => {
                let original = module
                    .aliases
                    .get(name.id.as_str())
                    .cloned()
                    .unwrap_or_else(|| name.id.to_string());
                let aliases = module
                    .aliases
                    .iter()
                    .filter(|(_, value)| **value == original)
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                module
                    .bindings
                    .insert(original, Binding::Refused(reason.into()));
                module
                    .bindings
                    .insert(name.id.to_string(), Binding::Refused(reason.into()));
                for alias in aliases {
                    module
                        .bindings
                        .insert(alias, Binding::Refused(reason.into()));
                }
                let mut invalidated = BTreeSet::from([name.id.to_string()]);
                loop {
                    let before = invalidated.len();
                    let dependents = module
                        .escaped
                        .iter()
                        .filter(|(_, sources)| {
                            sources.iter().any(|source| invalidated.contains(source))
                        })
                        .map(|(dependent, _)| dependent.clone())
                        .collect::<Vec<_>>();
                    invalidated.extend(dependents);
                    if before == invalidated.len() {
                        break;
                    }
                }
                for name in invalidated {
                    module
                        .bindings
                        .insert(name, Binding::Refused(reason.into()));
                }
            }
            ast::Expr::Subscript(subscript) => {
                Self::invalidate_target(module, &subscript.value, reason)
            }
            ast::Expr::Tuple(tuple) => {
                for item in &tuple.elts {
                    Self::invalidate_target(module, item, reason);
                }
            }
            ast::Expr::List(list) => {
                for item in &list.elts {
                    Self::invalidate_target(module, item, reason);
                }
            }
            _ => {}
        }
    }

    fn apply_effects(
        &self,
        module: &mut Module,
        expr: &ast::Expr,
        prefix: Option<&str>,
    ) -> Result<()> {
        let changed = effects(expr, &module.bindings)?;
        for name in changed {
            let target = if let Some(prefix) = prefix {
                let scoped = format!("{prefix}.{name}");
                if module.bindings.contains_key(&scoped) {
                    scoped
                } else {
                    name
                }
            } else {
                name
            };
            Self::invalidate_target(
                module,
                &ast::Expr::Name(ast::ExprName {
                    range: expr.range(),
                    id: target.into(),
                    ctx: ast::ExprContext::Load,
                }),
                "evaluated expression can mutate a table",
            );
        }
        Ok(())
    }

    fn invalidate_block(module: &mut Module, body: &[ast::Stmt]) {
        for name in super::effects::conditional_names(body) {
            if name == "*" {
                for binding in module.bindings.values_mut() {
                    *binding =
                        Binding::Refused("conditional wildcard import requires a hand port".into());
                }
            } else {
                module.bindings.insert(
                    name,
                    Binding::Refused("conditional rebinding requires a hand port".into()),
                );
            }
        }
        for stmt in body {
            match stmt {
                ast::Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        Self::invalidate_target(
                            module,
                            target,
                            "conditional assignment requires a hand port",
                        );
                    }
                }
                ast::Stmt::AnnAssign(assign) => Self::invalidate_target(
                    module,
                    &assign.target,
                    "conditional assignment requires a hand port",
                ),
                ast::Stmt::AugAssign(assign) => Self::invalidate_target(
                    module,
                    &assign.target,
                    "conditional mutation requires a hand port",
                ),
                ast::Stmt::Delete(delete) => {
                    for target in &delete.targets {
                        Self::invalidate_target(
                            module,
                            target,
                            "conditional deletion requires a hand port",
                        );
                    }
                }
                ast::Stmt::Expr(expr) => {
                    if let ast::Expr::Call(call) = &*expr.value
                        && let ast::Expr::Attribute(attr) = &*call.func
                    {
                        Self::invalidate_target(
                            module,
                            &attr.value,
                            "conditional method can mutate a table",
                        );
                    }
                }
                ast::Stmt::If(block) => {
                    Self::invalidate_block(module, &block.body);
                    Self::invalidate_block(module, &block.orelse);
                }
                ast::Stmt::For(block) => {
                    Self::invalidate_target(
                        module,
                        &block.target,
                        "loop binding requires a hand port",
                    );
                    Self::invalidate_block(module, &block.body);
                    Self::invalidate_block(module, &block.orelse);
                }
                ast::Stmt::AsyncFor(block) => {
                    Self::invalidate_target(
                        module,
                        &block.target,
                        "loop binding requires a hand port",
                    );
                    Self::invalidate_block(module, &block.body);
                    Self::invalidate_block(module, &block.orelse);
                }
                ast::Stmt::While(block) => {
                    Self::invalidate_block(module, &block.body);
                    Self::invalidate_block(module, &block.orelse);
                }
                ast::Stmt::With(block) => Self::invalidate_block(module, &block.body),
                ast::Stmt::AsyncWith(block) => Self::invalidate_block(module, &block.body),
                ast::Stmt::Try(block) => {
                    Self::invalidate_block(module, &block.body);
                    Self::invalidate_block(module, &block.orelse);
                    Self::invalidate_block(module, &block.finalbody);
                    for handler in &block.handlers {
                        let ast::ExceptHandler::ExceptHandler(handler) = handler;
                        Self::invalidate_block(module, &handler.body);
                    }
                }
                ast::Stmt::TryStar(block) => {
                    Self::invalidate_block(module, &block.body);
                    Self::invalidate_block(module, &block.orelse);
                    Self::invalidate_block(module, &block.finalbody);
                    for handler in &block.handlers {
                        let ast::ExceptHandler::ExceptHandler(handler) = handler;
                        Self::invalidate_block(module, &handler.body);
                    }
                }
                ast::Stmt::Match(block) => {
                    for case in &block.cases {
                        Self::invalidate_block(module, &case.body);
                    }
                }
                _ => {}
            }
        }
    }

    fn class_binding(
        &mut self,
        class: &ast::StmtClassDef,
        bindings: &Bindings,
        path: &str,
    ) -> Result<Binding> {
        let is_enum = class.bases.iter().any(|base| {
            expression_path(base).is_some_and(|name| name == "Enum" || name.ends_with(".Enum"))
        });
        if is_enum {
            let mut values = BTreeMap::new();
            for stmt in &class.body {
                if let ast::Stmt::Assign(assign) = stmt {
                    for target in &assign.targets {
                        if let ast::Expr::Name(name) = target {
                            values.insert(
                                name.id.to_string(),
                                self.eval_binding(&assign.value, bindings, path, 0)?,
                            );
                        }
                    }
                }
            }
            return Ok(Binding::Namespace(values));
        }
        let is_dataclass = class.decorator_list.iter().any(|decorator| {
            let decorator = if let ast::Expr::Call(call) = decorator {
                &*call.func
            } else {
                decorator
            };
            expression_path(decorator)
                .is_some_and(|name| name == "dataclass" || name.ends_with(".dataclass"))
        });
        ensure!(
            is_dataclass,
            "class {} is not a literal dataclass or enum",
            class.name
        );
        let mut fields = Vec::new();
        for stmt in &class.body {
            if let ast::Stmt::AnnAssign(field) = stmt {
                let ast::Expr::Name(name) = &*field.target else {
                    bail!("non-name dataclass field");
                };
                if expression_path(&field.annotation).is_some_and(|name| name == "ClassVar") {
                    continue;
                }
                let default = field
                    .value
                    .as_ref()
                    .map(|value| self.eval(value, bindings, path, 0))
                    .transpose()?;
                fields.push((name.id.to_string(), default));
            }
        }
        Ok(Binding::Constructor(fields))
    }

    fn collect_defaults(
        &mut self,
        body: &[ast::Stmt],
        prefix: &str,
        bindings: &Bindings,
        module: &mut Module,
        source: &str,
        path: &str,
    ) -> Result<()> {
        for stmt in body {
            match stmt {
                ast::Stmt::FunctionDef(function) => self.function_defaults(
                    &format!("{prefix}.{}", function.name),
                    &function.args,
                    stmt,
                    bindings,
                    module,
                    source,
                    path,
                )?,
                ast::Stmt::AsyncFunctionDef(function) => self.function_defaults(
                    &format!("{prefix}.{}", function.name),
                    &function.args,
                    stmt,
                    bindings,
                    module,
                    source,
                    path,
                )?,
                ast::Stmt::AnnAssign(field) => {
                    if let (ast::Expr::Name(name), Some(value)) = (&*field.target, &field.value) {
                        self.apply_effects(module, value, Some(prefix))?;
                        let key = format!("{prefix}.{}", name.id);
                        module
                            .escaped
                            .insert(key.clone(), dependencies(value, bindings)?);
                        let binding = self
                            .eval_binding(value, bindings, path, 0)
                            .unwrap_or_else(|error| Binding::Refused(format!("{error:#}")));
                        module.bindings.insert(key.clone(), binding);
                        module.locations.insert(
                            key,
                            (
                                line(source, stmt.range().start().to_usize()),
                                line(source, stmt.range().end().to_usize()),
                            ),
                        );
                    }
                }
                ast::Stmt::Assign(field) => {
                    self.apply_effects(module, &field.value, Some(prefix))?;
                    for target in &field.targets {
                        if let ast::Expr::Name(name) = target {
                            let key = format!("{prefix}.{}", name.id);
                            module
                                .escaped
                                .insert(key.clone(), dependencies(&field.value, bindings)?);
                            let evaluated = if path
                                == "sdk/python/bittensor/intents/hyperparameters.py"
                                && key == "SetHyperparameter.wraps"
                            {
                                let (start, end) = module
                                    .locations
                                    .get("OWNER_HYPERPARAMETERS")
                                    .copied()
                                    .unwrap_or((0, 0));
                                module.derived.insert(key.clone(),json!({"kind":"unique_owner_setters","input":{"path":path,"table":"OWNER_HYPERPARAMETERS","line":start,"end_line":end,"blob":self.tree.get(path)}}));
                                derive_hyperparam_wraps(&field.value, bindings)
                            } else {
                                self.eval_binding(&field.value, bindings, path, 0)
                            };
                            let binding = evaluated
                                .unwrap_or_else(|error| Binding::Refused(format!("{error:#}")));
                            module.bindings.insert(key.clone(), binding);
                            module.locations.insert(
                                key,
                                (
                                    line(source, stmt.range().start().to_usize()),
                                    line(source, stmt.range().end().to_usize()),
                                ),
                            );
                        }
                    }
                }
                ast::Stmt::Expr(expr) => self.apply_effects(module, &expr.value, Some(prefix))?,
                _ => {
                    let mut local = Module {
                        bindings: BTreeMap::new(),
                        locations: BTreeMap::new(),
                        aliases: BTreeMap::new(),
                        escaped: BTreeMap::new(),
                        derived: BTreeMap::new(),
                    };
                    Self::invalidate_block(&mut local, std::slice::from_ref(stmt));
                    for (name, binding) in local.bindings {
                        let key = format!("{prefix}.{name}");
                        if module.bindings.contains_key(&key) {
                            module.bindings.insert(key, binding);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn function_defaults(
        &mut self,
        prefix: &str,
        args: &ast::Arguments,
        stmt: &ast::Stmt,
        bindings: &Bindings,
        module: &mut Module,
        source: &str,
        path: &str,
    ) -> Result<()> {
        for argument in args
            .posonlyargs
            .iter()
            .chain(&args.args)
            .chain(&args.kwonlyargs)
        {
            if let Some(default) = &argument.default {
                self.apply_effects(
                    module,
                    default,
                    if prefix.contains('.') {
                        prefix.split('.').next()
                    } else {
                        None
                    },
                )?;
                let key = format!("{prefix}.{}", argument.def.arg);
                let binding = self
                    .eval_binding(default, bindings, path, 0)
                    .unwrap_or_else(|error| Binding::Refused(format!("{error:#}")));
                module.bindings.insert(key.clone(), binding);
                module.locations.insert(
                    key,
                    (
                        line(source, stmt.range().start().to_usize()),
                        line(source, stmt.range().end().to_usize()),
                    ),
                );
            }
        }
        Ok(())
    }

    fn eval_binding(
        &mut self,
        expr: &ast::Expr,
        bindings: &Bindings,
        path: &str,
        depth: usize,
    ) -> Result<Binding> {
        ensure!(depth < MAX_DEPTH, "name resolution exceeds depth limit");
        match expr {
            ast::Expr::Name(name) if bindings.contains_key(name.id.as_str()) => {
                clone_binding(&bindings[name.id.as_str()], &self.budget)
            }
            ast::Expr::Attribute(attr) => {
                if path == "sdk/python/tests/harness/samples.py"
                    && attr.attr.as_str() == "ss58_address"
                {
                    return Ok(Binding::Value(self.eval(expr, bindings, path, depth)?));
                }
                let base = self.eval_binding(&attr.value, bindings, path, depth + 1)?;
                match base {
                    Binding::Namespace(namespace) => namespace
                        .get(attr.attr.as_str())
                        .map(|binding| clone_binding(binding, &self.budget))
                        .transpose()?
                        .context("enum attribute does not exist"),
                    Binding::Module(module)
                        if module.starts_with("sdk/") && !module.contains("/_generated/") =>
                    {
                        self.module(&module)?;
                        self.modules[&module]
                            .bindings
                            .get(attr.attr.as_str())
                            .map(|binding| clone_binding(binding, &self.budget))
                            .transpose()?
                            .context("imported module attribute does not exist")
                    }
                    Binding::Module(module) | Binding::Symbol(module) => {
                        Ok(Binding::Symbol(format!("{module}.{}", attr.attr)))
                    }
                    _ => bail!("attribute base is not an accepted namespace"),
                }
            }
            _ => Ok(Binding::Value(self.eval(expr, bindings, path, depth)?)),
        }
    }

    fn eval(
        &mut self,
        expr: &ast::Expr,
        bindings: &Bindings,
        path: &str,
        depth: usize,
    ) -> Result<Literal> {
        ensure!(depth < MAX_DEPTH, "resolved expression exceeds depth limit");
        spend(&self.budget, 64)?;
        let next = depth + 1;
        let value = match expr {
            ast::Expr::Constant(value) => constant(&value.value)?,
            ast::Expr::Name(name) => match bindings.get(name.id.as_str()) {
                Some(Binding::Value(value)) => value.clone(),
                Some(Binding::Refused(reason)) => bail!("cannot resolve {}: {reason}", name.id),
                Some(_) => bail!("{} is not a resolved value", name.id),
                None if matches!(name.id.as_str(), "str" | "int" | "float" | "bool") => {
                    Literal::Resolved(json!({"type":name.id.as_str()}))
                }
                None => bail!("unresolved name {}", name.id),
            },
            ast::Expr::List(value) => {
                Literal::List(self.sequence(&value.elts, bindings, path, next)?)
            }
            ast::Expr::Tuple(value) => {
                Literal::Tuple(self.sequence(&value.elts, bindings, path, next)?)
            }
            ast::Expr::Set(value) => Literal::Set(unique_hashable(self.sequence(
                &value.elts,
                bindings,
                path,
                next,
            )?)?),
            ast::Expr::Dict(value) => {
                ensure!(
                    value.keys.len() <= MAX_CONTAINER_ITEMS,
                    "dictionary exceeds item limit"
                );
                let mut entries = Vec::new();
                for (key, value) in value.keys.iter().zip(&value.values) {
                    let key = self.eval(
                        key.as_ref()
                            .context("dictionary unpacking requires a hand port")?,
                        bindings,
                        path,
                        next,
                    )?;
                    ensure!(key.hashable(), "dictionary key is not hashable");
                    insert_entry(&mut entries, key, self.eval(value, bindings, path, next)?);
                }
                Literal::Dict(entries)
            }
            ast::Expr::UnaryOp(value) => {
                match (value.op, self.eval(&value.operand, bindings, path, next)?) {
                    (ast::UnaryOp::UAdd, value @ (Literal::Int(_) | Literal::Float(_))) => value,
                    (ast::UnaryOp::USub, Literal::Int(value)) => super::checked_int(-value)?,
                    (ast::UnaryOp::USub, Literal::Float(value)) => super::checked_float(-value)?,
                    _ => bail!("unary operator requires numeric literal"),
                }
            }
            ast::Expr::BinOp(value) => {
                let left = self.eval(&value.left, bindings, path, next)?;
                let right = self.eval(&value.right, bindings, path, next)?;
                if value.op == ast::Operator::Div {
                    if let (Literal::Resolved(home), Literal::Str(segment)) = (&left, &right) {
                        ensure!(
                            home.as_object().is_some_and(|value| value.len() == 1),
                            "path division requires a Path, not its string representation"
                        );
                        let mut segments = home
                            .get("home_relative")
                            .and_then(Value::as_array)
                            .context("only Path.home() supports path division")?
                            .clone();
                        ensure!(
                            !segment.starts_with('/'),
                            "home-relative path segment must not be absolute"
                        );
                        segments.push(json!(segment));
                        Literal::Resolved(json!({"home_relative":segments}))
                    } else {
                        binary(value.op, left, right, &self.budget)?
                    }
                } else {
                    binary(value.op, left, right, &self.budget)?
                }
            }
            ast::Expr::BoolOp(value) if value.op == ast::BoolOp::Or && value.values.len() == 2 => {
                let call = match &value.values[0] {
                    ast::Expr::Call(call) => call,
                    _ => bail!("only environment-default or expressions are accepted"),
                };
                self.environment(call, Some(&value.values[1]), bindings, path, next)?
            }
            ast::Expr::Call(call) => self.call(call, bindings, path, next)?,
            ast::Expr::Attribute(attribute)
                if path == "sdk/python/tests/harness/samples.py"
                    && attribute.attr.as_str() == "ss58_address" =>
            {
                let ast::Expr::Call(call) = &*attribute.value else {
                    bail!("dev address must use a fixed URI constructor");
                };
                ensure!(
                    call_path(&call.func).as_deref() == Some("Keypair.create_from_uri")
                        && call.args.len() == 1
                        && call.keywords.is_empty(),
                    "test-harness address must use the fixed dev URI table"
                );
                let Literal::Str(uri) = self.eval(&call.args[0], bindings, path, next)? else {
                    bail!("dev URI must be a literal string");
                };
                ensure!(
                    matches!(
                        uri.as_str(),
                        "//Alice" | "//Alice//hot" | "//Bob" | "//Bob//hot"
                    ),
                    "dev URI is absent from the fixed table"
                );
                Literal::Str(
                    bittensor_core::keys::Keypair::from_uri(
                        &uri,
                        bittensor_core::keys::CRYPTO_SR25519,
                    )?
                    .ss58_address(),
                )
            }
            ast::Expr::Attribute(_) => match self.eval_binding(expr, bindings, path, next)? {
                Binding::Value(value) => value,
                Binding::Symbol(symbol) if symbol == "re.IGNORECASE" => {
                    Literal::Resolved(json!({"enum":"re.IGNORECASE"}))
                }
                Binding::Symbol(symbol)
                    if symbol.contains("/_generated/storage.")
                        || symbol.contains("/_generated/runtime_apis.") =>
                {
                    self.descriptor(&symbol)?
                }
                _ => bail!("attribute is not a resolved enum or generated descriptor"),
            },
            ast::Expr::Subscript(value) => {
                let container = self.eval(&value.value, bindings, path, next)?;
                let key = self.eval(&value.slice, bindings, path, next)?;
                match container {
                    Literal::Dict(entries) => {
                        entries
                            .into_iter()
                            .find(|(candidate, _)| equal_key(candidate, &key))
                            .context("dictionary key does not exist")?
                            .1
                    }
                    _ => bail!("only literal dictionary subscripts are accepted"),
                }
            }
            _ => bail!("unsupported Python expression; extraction never executes source code"),
        };
        let units = value.allocation_units(depth)?;
        spend(&self.budget, units)?;
        Ok(value)
    }

    fn sequence(
        &mut self,
        expressions: &[ast::Expr],
        bindings: &Bindings,
        path: &str,
        depth: usize,
    ) -> Result<Vec<Literal>> {
        ensure!(
            expressions.len() <= MAX_CONTAINER_ITEMS,
            "container exceeds item limit"
        );
        expressions
            .iter()
            .map(|expr| self.eval(expr, bindings, path, depth))
            .collect()
    }

    fn symbol(
        &mut self,
        expr: &ast::Expr,
        bindings: &Bindings,
        path: &str,
        depth: usize,
    ) -> Result<Binding> {
        match expr {
            ast::Expr::Name(name) if !bindings.contains_key(name.id.as_str()) => {
                Ok(Binding::Symbol(name.id.to_string()))
            }
            _ => self.eval_binding(expr, bindings, path, depth),
        }
    }

    fn call(
        &mut self,
        call: &ast::ExprCall,
        bindings: &Bindings,
        path: &str,
        depth: usize,
    ) -> Result<Literal> {
        let binding = self.symbol(&call.func, bindings, path, depth)?;
        if let Binding::Constructor(fields) = binding {
            ensure!(
                call.args.len() <= fields.len(),
                "too many dataclass positional arguments"
            );
            let mut values = BTreeMap::new();
            for ((name, _), expr) in fields.iter().zip(&call.args) {
                values.insert(name.clone(), self.eval(expr, bindings, path, depth)?);
            }
            for keyword in &call.keywords {
                let name = keyword
                    .arg
                    .as_ref()
                    .context("keyword unpacking requires a hand port")?
                    .to_string();
                ensure!(
                    fields.iter().any(|(field, _)| *field == name),
                    "unknown dataclass field {name}"
                );
                ensure!(
                    !values.contains_key(&name),
                    "dataclass field {name} provided twice"
                );
                values.insert(name, self.eval(&keyword.value, bindings, path, depth)?);
            }
            for (name, default) in fields {
                if !values.contains_key(&name) {
                    values.insert(
                        name.clone(),
                        default.with_context(|| format!("missing dataclass field {name}"))?,
                    );
                }
            }
            let fields = values
                .into_iter()
                .map(|(name, value)| Ok((name, value.json()?)))
                .collect::<Result<serde_json::Map<_, _>>>()?;
            return Ok(Literal::Resolved(
                json!({"constructor":call_path(&call.func),"fields":fields}),
            ));
        }
        let Binding::Symbol(symbol) = binding else {
            bail!("call is not an accepted builtin or dataclass constructor");
        };
        if matches!(symbol.as_str(), "os.getenv" | "os.environ.get") {
            return self.environment(call, None, bindings, path, depth);
        }
        if matches!(symbol.as_str(), "pathlib.Path.home" | "Path.home") {
            ensure!(
                call.args.is_empty() && call.keywords.is_empty(),
                "Path.home accepts no arguments"
            );
            return Ok(Literal::Resolved(json!({"home_relative":[]})));
        }
        if symbol == "sdk/python/bittensor/_generated/storage.py.Item" {
            ensure!(
                (2..=3).contains(&call.args.len()) && call.keywords.is_empty(),
                "storage Item requires two names and an optional type identity"
            );
            let args = call
                .args
                .iter()
                .map(|expr| self.eval(expr, bindings, path, depth))
                .collect::<Result<Vec<_>>>()?;
            let Literal::Str(pallet) = &args[0] else {
                bail!("storage pallet must be a string");
            };
            let Literal::Str(item) = &args[1] else {
                bail!("storage item must be a string");
            };
            if let Some(identity) = args.get(2) {
                ensure!(
                    matches!(identity, Literal::Str(_)),
                    "storage value identity must be a string"
                );
            }
            let identity = self.validate_descriptor(pallet, item, false)?;
            return Ok(Literal::Resolved(
                json!({"pallet":pallet,"item":item,"value_type_ident":identity,"source_value_type_ident":args.get(2).map(Literal::json).transpose()?}),
            ));
        }
        if matches!(symbol.as_str(), "dataclasses.field" | "field") {
            ensure!(
                call.args.is_empty(),
                "field positional arguments are unsupported"
            );
            let default = call
                .keywords
                .iter()
                .find(|keyword| keyword.arg.as_ref().is_some_and(|name| name == "default"));
            let factory = call.keywords.iter().find(|keyword| {
                keyword
                    .arg
                    .as_ref()
                    .is_some_and(|name| name == "default_factory")
            });
            ensure!(
                !(default.is_some() && factory.is_some()),
                "field provides both default and default_factory"
            );
            if let Some(default) = default {
                return self.eval(&default.value, bindings, path, depth);
            }
            if let Some(factory) = factory {
                if let ast::Expr::Name(name) = &factory.value {
                    ensure!(
                        !bindings.contains_key(name.id.as_str()),
                        "default_factory builtin is shadowed"
                    );
                    return match name.id.as_str() {
                        "list" => Ok(Literal::List(Vec::new())),
                        "dict" => Ok(Literal::Dict(Vec::new())),
                        "tuple" => Ok(Literal::Tuple(Vec::new())),
                        "set" => Ok(Literal::Set(Vec::new())),
                        _ => bail!("default_factory is not an accepted builtin constructor"),
                    };
                }
                let ast::Expr::Lambda(lambda) = &factory.value else {
                    bail!("default_factory must be a zero-argument lambda");
                };
                ensure!(
                    lambda.args.posonlyargs.is_empty()
                        && lambda.args.args.is_empty()
                        && lambda.args.kwonlyargs.is_empty()
                        && lambda.args.vararg.is_none()
                        && lambda.args.kwarg.is_none(),
                    "default_factory lambda must take no arguments"
                );
                return self.eval(&lambda.body, bindings, path, depth);
            }
            return Ok(Literal::Resolved(json!({"required":true})));
        }
        if symbol == "str" {
            ensure!(
                call.args.len() == 1 && call.keywords.is_empty(),
                "path string requires one bounded home-relative argument"
            );
            let Literal::Resolved(mut home) = self.eval(&call.args[0], bindings, path, depth)?
            else {
                bail!("str extraction accepts only a home-relative path");
            };
            ensure!(
                home.as_object().is_some_and(|value| value.len() == 1
                    && value.get("home_relative").is_some_and(Value::is_array)),
                "str extraction accepts only Path.home() path expressions"
            );
            home["path_string"] = json!(true);
            return Ok(Literal::Resolved(home));
        }
        if symbol == "re.compile" {
            ensure!(
                (1..=2).contains(&call.args.len()) && call.keywords.is_empty(),
                "re.compile requires pattern and optional accepted flags"
            );
            let pattern = self.eval(&call.args[0], bindings, path, depth)?;
            ensure!(
                matches!(pattern, Literal::Str(_)),
                "regular expression pattern must be a string"
            );
            let flags = call
                .args
                .get(1)
                .map(|expr| self.eval(expr, bindings, path, depth))
                .transpose()?;
            return Ok(Literal::Resolved(
                json!({"regex":pattern.json()?,"flags":flags.map(|flags| flags.json()).transpose()?}),
            ));
        }
        ensure!(
            matches!(
                symbol.as_str(),
                "frozenset" | "set" | "list" | "tuple" | "dict" | "bytes"
            ),
            "constructor {symbol:?} is not accepted"
        );
        let mut local = super::Bindings::new();
        let mut plain = call.clone();
        for (index, arg) in plain.args.iter_mut().enumerate() {
            let name = format!("__rtao_argument_{index}");
            local.insert(
                name.clone(),
                super::Binding::Value(self.eval(arg, bindings, path, depth)?),
            );
            *arg = ast::Expr::Name(ast::ExprName {
                range: arg.range(),
                id: name.into(),
                ctx: ast::ExprContext::Load,
            });
        }
        for (index, keyword) in plain.keywords.iter_mut().enumerate() {
            let name = format!("__rtao_keyword_{index}");
            local.insert(
                name.clone(),
                super::Binding::Value(self.eval(&keyword.value, bindings, path, depth)?),
            );
            keyword.value = ast::Expr::Name(ast::ExprName {
                range: keyword.value.range(),
                id: name.into(),
                ctx: ast::ExprContext::Load,
            });
        }
        // The original evaluator already checks duplicate keys, hashability,
        // unordered conversion and resource limits for builtin constructors.
        super::Evaluator {
            bindings: &local,
            budget: &self.budget,
        }
        .constructor(&plain, depth)
    }

    fn environment(
        &mut self,
        call: &ast::ExprCall,
        fallback: Option<&ast::Expr>,
        bindings: &Bindings,
        path: &str,
        depth: usize,
    ) -> Result<Literal> {
        let Binding::Symbol(symbol) = self.symbol(&call.func, bindings, path, depth)? else {
            bail!("environment expression is not os.getenv or os.environ.get");
        };
        ensure!(
            matches!(symbol.as_str(), "os.getenv" | "os.environ.get"),
            "environment expression is not os.getenv or os.environ.get"
        );
        ensure!(
            call.keywords.is_empty() && (1..=2).contains(&call.args.len()),
            "environment call accepts one name and one optional default"
        );
        let Literal::Str(name) = self.eval(&call.args[0], bindings, path, depth)? else {
            bail!("environment variable name must be a literal string");
        };
        let default = if let Some(fallback) = fallback {
            ensure!(
                call.args.len() == 1,
                "getenv-or form must not supply another default"
            );
            self.eval(fallback, bindings, path, depth)?
        } else if let Some(default) = call.args.get(1) {
            self.eval(default, bindings, path, depth)?
        } else {
            Literal::None
        };
        Ok(Literal::Resolved(
            json!({"env":name,"default":default.json()?,"empty_is_unset":fallback.is_some()}),
        ))
    }

    fn validate_descriptor(
        &mut self,
        group: &str,
        item: &str,
        runtime_api: bool,
    ) -> Result<Option<String>> {
        if self.surface.is_none() {
            let pins =
                fs::read_to_string(self.root.join("upstream.toml"))?.parse::<DocumentMut>()?;
            crate::metadata_pin::verify(self.root, &pins)
                .context("verify pinned metadata before resolving descriptors")?;
            let path = pins["networks"]["finney"]["metadata"]["v15"]
                .as_str()
                .context("missing pinned V15 metadata")?;
            let bytes = fs::read(self.root.join(path))?;
            let metadata = RuntimeMetadataPrefixed::decode_all(&mut &bytes[..])?;
            let spec = u32::try_from(
                pins["networks"]["finney"]["metadata"]["spec"]
                    .as_integer()
                    .context("missing metadata spec")?,
            )?;
            let runtime = bittensor_core::runtime::Runtime::parse(
                &bytes,
                spec,
                0,
                bittensor_core::keys::DEFAULT_SS58_FORMAT,
            )?;
            let RuntimeMetadata::V15(metadata) = metadata.1 else {
                bail!("descriptor resolution requires pinned V15 metadata");
            };
            let mut storage = BTreeMap::new();
            for pallet in metadata.pallets {
                if let Some(entries) = pallet.storage {
                    for entry in entries.entries {
                        let identity = match entry.ty {
                            frame_metadata::v14::StorageEntryType::Plain(ty) => ty.id,
                            frame_metadata::v14::StorageEntryType::Map { value, .. } => value.id,
                        };
                        storage.insert(
                            (pallet.name.clone(), entry.name),
                            runtime.type_ident(identity),
                        );
                    }
                }
            }
            let apis = metadata
                .apis
                .into_iter()
                .flat_map(|api| {
                    let name = api.name;
                    api.methods
                        .into_iter()
                        .map(move |method| (name.clone(), method.name))
                })
                .collect();
            self.surface = Some((storage, apis));
        }
        let (storage, apis) = self
            .surface
            .as_ref()
            .context("descriptor surface missing")?;
        let key = (group.into(), item.into());
        if runtime_api {
            ensure!(
                apis.contains(&key),
                "descriptor {group}.{item} is absent from pinned metadata"
            );
            Ok(None)
        } else {
            Ok(Some(
                storage
                    .get(&key)
                    .with_context(|| {
                        format!("descriptor {group}.{item} is absent from pinned metadata")
                    })?
                    .clone(),
            ))
        }
    }

    fn descriptor(&mut self, symbol: &str) -> Result<Literal> {
        let (module, descriptor) = symbol
            .split_once(".py.")
            .or_else(|| {
                symbol
                    .split_once("storage.")
                    .map(|(_, rest)| ("sdk/python/bittensor/_generated/storage", rest))
            })
            .or_else(|| {
                symbol
                    .split_once("runtime_apis.")
                    .map(|(_, rest)| ("sdk/python/bittensor/_generated/runtime_apis", rest))
            })
            .context("descriptor has no module prefix")?;
        let (group, item) = descriptor
            .split_once('.')
            .context("descriptor must have exactly a group and an item")?;
        ensure!(
            !item.contains('.'),
            "descriptor chain has too many attributes"
        );
        let module = module.trim_end_matches(".py");
        let source = self.source(&format!("{module}.py"))?;
        let suite = ast::Suite::parse(&source, module)
            .map_err(|error| anyhow::anyhow!("{module}: {error}"))?;
        let declaration = suite.iter().find_map(|stmt| match stmt {
            ast::Stmt::ClassDef(class) if class.name.as_str() == group => class.body.iter().find_map(|stmt| match stmt {
                ast::Stmt::Assign(assign) if assign.targets.iter().any(|target|matches!(target,ast::Expr::Name(name) if name.id.as_str() == item)) => Some(&*assign.value),
                ast::Stmt::AnnAssign(assign) if matches!(&*assign.target,ast::Expr::Name(name) if name.id.as_str() == item) => assign.value.as_deref(),
                _ => None,
            }),
            _ => None,
        });
        let declaration = declaration.with_context(|| {
            format!("generated descriptor {group}.{item} is absent from pinned upstream surface")
        })?;
        let ast::Expr::Call(call) = declaration else {
            bail!("generated descriptor {group}.{item} must be a literal constructor");
        };
        let api = module.ends_with("runtime_apis");
        ensure!(
            call_path(&call.func).as_deref() == Some(if api { "Method" } else { "Item" })
                && call.keywords.is_empty()
                && call.args.len() == if api { 2 } else { 3 },
            "generated descriptor has a changed constructor"
        );
        ensure!(
            matches!(&call.args[0],ast::Expr::Constant(value) if matches!(&value.value,ast::Constant::Str(value) if value == group))
                && matches!(&call.args[1],ast::Expr::Constant(value) if matches!(&value.value,ast::Constant::Str(value) if value == item)),
            "generated descriptor {group}.{item} payload changes its target"
        );
        let identity = self.validate_descriptor(group, item, api)?;
        let source_identity = if api {
            None
        } else {
            let ast::Expr::Constant(value) = &call.args[2] else {
                bail!("storage identity must be literal");
            };
            let ast::Constant::Str(value) = &value.value else {
                bail!("storage identity must be string");
            };
            Some(value)
        };
        Ok(Literal::Resolved(if module.ends_with("storage") {
            json!({"pallet":group,"item":item,"value_type_ident":identity,"source_value_type_ident":source_identity})
        } else {
            json!({"api":group,"method":item})
        }))
    }

    fn values(&mut self, path: &str, names: &[&str]) -> Result<Value> {
        self.module(path)?;
        let module = &self.modules[path];
        let mut values = serde_json::Map::new();
        for name in names {
            let binding = module
                .bindings
                .get(*name)
                .with_context(|| format!("{path}: required table {name} has no assignment"))?;
            let Binding::Value(value) = binding else {
                if let Binding::Refused(reason) = binding {
                    bail!("{path}: required table {name}: {reason}");
                }
                bail!("{path}: required table {name} is not a resolved value");
            };
            let (start, end) = module
                .locations
                .get(*name)
                .with_context(|| format!("{path}: table {name} has no source span"))?;
            let mut extracted = json!({"line":start,"end_line":end,"value":value.json()?});
            if let Some(derived) = module.derived.get(*name) {
                extracted["derived"] = derived.clone();
            }
            values.insert((*name).into(), extracted);
        }
        Ok(Value::Object(values))
    }
}

fn expression_path(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Name(name) => Some(name.id.to_string()),
        ast::Expr::Attribute(attr) => {
            Some(format!("{}.{}", expression_path(&attr.value)?, attr.attr))
        }
        ast::Expr::Subscript(subscript) => expression_path(&subscript.value),
        _ => None,
    }
}

fn call_path(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Name(name) => Some(name.id.to_string()),
        ast::Expr::Attribute(attr) => Some(format!("{}.{}", call_path(&attr.value)?, attr.attr)),
        _ => None,
    }
}

fn one_argument(call: &ast::ExprCall) -> Result<&ast::Expr> {
    ensure!(
        call.args.len() == 1 && call.keywords.is_empty(),
        "derived table constructor must have exactly one positional argument"
    );
    Ok(&call.args[0])
}

/// This is the hand-ported derivation named by the architecture's derived-table
/// rule. The evaluator continues to reject comprehensions and generators.
fn derive_hyperparam_wraps(expr: &ast::Expr, bindings: &Bindings) -> Result<Binding> {
    let ast::Expr::Call(tuple) = expr else {
        bail!("derived wraps must call tuple");
    };
    ensure!(
        call_path(&tuple.func).as_deref() == Some("tuple") && !bindings.contains_key("tuple"),
        "derived wraps must use the builtin tuple"
    );
    let ast::Expr::Call(fromkeys) = one_argument(tuple)? else {
        bail!("derived wraps must call dict.fromkeys");
    };
    ensure!(
        call_path(&fromkeys.func).as_deref() == Some("dict.fromkeys")
            && !bindings.contains_key("dict"),
        "derived wraps must use builtin dict.fromkeys"
    );
    let ast::Expr::GeneratorExp(generator) = one_argument(fromkeys)? else {
        bail!("derived wraps must use the owner-setter generator");
    };
    ensure!(
        generator.generators.len() == 1,
        "derived wraps requires exactly one generator"
    );
    let iteration = &generator.generators[0];
    ensure!(
        !iteration.is_async && iteration.ifs.is_empty(),
        "derived wraps must not filter or await"
    );
    let ast::Expr::Tuple(target) = &iteration.target else {
        bail!("derived wraps must unpack a setter and a flag");
    };
    ensure!(
        target.elts.len() == 2,
        "derived wraps must unpack exactly two values"
    );
    let (ast::Expr::Name(method), ast::Expr::Name(flag)) = (&target.elts[0], &target.elts[1])
    else {
        bail!("derived wraps unpack variables must be names");
    };
    ensure!(
        method.id != flag.id,
        "derived wraps unpack names must differ"
    );
    let ast::Expr::Call(values) = &iteration.iter else {
        bail!("derived wraps must iterate owner-setter values");
    };
    ensure!(
        call_path(&values.func).as_deref() == Some("OWNER_HYPERPARAMETERS.values")
            && values.args.is_empty()
            && values.keywords.is_empty(),
        "derived wraps must iterate OWNER_HYPERPARAMETERS.values()"
    );
    let ast::Expr::Tuple(pair) = &*generator.elt else {
        bail!("derived wraps must emit a tuple");
    };
    ensure!(
        pair.elts.len() == 2,
        "derived wraps must emit a two-item tuple"
    );
    ensure!(
        matches!(&pair.elts[0],ast::Expr::Constant(value) if matches!(&value.value,ast::Constant::Str(pallet) if pallet == "AdminUtils")),
        "derived wraps pallet must be AdminUtils"
    );
    ensure!(
        matches!(&pair.elts[1],ast::Expr::Name(name) if name.id == method.id),
        "derived wraps must emit the unpacked setter name"
    );
    let Some(Binding::Value(Literal::Dict(owners))) = bindings.get("OWNER_HYPERPARAMETERS") else {
        bail!("derived wraps needs the accepted OWNER_HYPERPARAMETERS table");
    };
    ensure!(
        owners.len() <= MAX_CONTAINER_ITEMS,
        "owner-setter table exceeds item limit"
    );
    let mut wraps = Vec::new();
    for (_, value) in owners {
        let Literal::Tuple(pair) = value else {
            bail!("owner-setter row is not a tuple");
        };
        ensure!(
            pair.len() == 2 && matches!(pair[1], Literal::Bool(_)),
            "owner-setter row must contain a setter and bool"
        );
        let Literal::Str(method) = &pair[0] else {
            bail!("owner setter is not a string");
        };
        let pair = Literal::Tuple(vec![
            Literal::Str("AdminUtils".into()),
            Literal::Str(method.clone()),
        ]);
        if !wraps.iter().any(|prior| equal_key(prior, &pair)) {
            wraps.push(pair);
        }
    }
    Ok(Binding::Value(Literal::Tuple(wraps)))
}

fn intent_classes(source: &str, path: &str) -> Result<BTreeSet<String>> {
    let suite =
        ast::Suite::parse(source, path).map_err(|error| anyhow::anyhow!("{path}: {error}"))?;
    let mut classes = BTreeSet::from(["Intent".to_owned()]);
    loop {
        let before = classes.len();
        for stmt in &suite {
            if let ast::Stmt::ClassDef(class) = stmt
                && class
                    .bases
                    .iter()
                    .any(|base| expression_path(base).is_some_and(|base| classes.contains(&base)))
            {
                classes.insert(class.name.to_string());
            }
        }
        if before == classes.len() {
            break;
        }
    }
    Ok(classes)
}

fn document(
    resolver: &mut Resolver<'_>,
    pins: &DocumentMut,
    path: &str,
    names: &[&str],
) -> Result<Value> {
    Ok(json!({
        "format":1,
        "provenance":{"tag":pins["parity"]["tag"].as_str().context("missing parity.tag")?,"commit":pins["parity"]["commit"].as_str().context("missing parity.commit")?,"path":path,"blob":resolver.tree.get(path).context("missing source blob")?},
        "values":resolver.values(path,names)?,
    }))
}

fn rpc_diagnostic_limits(resolver: &Resolver<'_>, document: &mut Value) -> Result<()> {
    let source = resolver.source(RPC)?;
    let suite =
        ast::Suite::parse(&source, RPC).map_err(|error| anyhow::anyhow!("{RPC}: {error}"))?;
    let mut rows = BTreeMap::new();
    for (function, name, body_slice) in [
        ("_response_header", "POLICY_METADATA_LIMIT", false),
        ("_response_detail", "POLICY_DETAIL_LIMIT", false),
        ("_response_detail", "POLICY_BODY_LIMIT", true),
    ] {
        let function = suite
            .iter()
            .find_map(|stmt| match stmt {
                ast::Stmt::FunctionDef(def) if def.name.as_str() == function => Some(def),
                _ => None,
            })
            .with_context(|| format!("{RPC}: missing diagnostic helper {function}"))?;
        let mut candidates = Vec::new();
        for stmt in &function.body {
            let expression = match stmt {
                ast::Stmt::Return(ret) => ret.value.as_deref(),
                ast::Stmt::Assign(assign) => Some(&*assign.value),
                _ => None,
            };
            if let Some(expression) = expression {
                super::effects::walk(expression, 0, &mut |expr| {
                    if body_slice {
                        if let ast::Expr::Subscript(subscript) = expr
                            && matches!(&*subscript.value,ast::Expr::Name(name) if name.id.as_str()=="body")
                            && let ast::Expr::Slice(slice) = &*subscript.slice
                            && slice.lower.is_none()
                            && slice.step.is_none()
                            && let Some(upper) = &slice.upper
                        {
                            candidates.push((upper.clone(), expr.range()));
                        }
                    } else if let ast::Expr::Call(call) = expr
                        && call_path(&call.func).as_deref() == Some("_safe_diagnostic")
                        && call.args.len() == 1
                        && call.keywords.len() == 1
                        && let Some(keyword) = call.keywords.first()
                        && keyword
                            .arg
                            .as_ref()
                            .is_some_and(|name| name.as_str() == "limit")
                    {
                        candidates.push((Box::new(keyword.value.clone()), expr.range()));
                    }
                })?;
            }
        }
        ensure!(
            candidates.len() == 1,
            "{RPC}:{}: diagnostic {name} must have one exact bounded AST use",
            line(&source, function.range.start().to_usize())
        );
        let (expr, span) = candidates.pop().context("missing diagnostic expression")?;
        let budget = Cell::new(MAX_ALLOCATION_UNITS);
        let literal = super::Evaluator {
            bindings: &super::Bindings::new(),
            budget: &budget,
        }
        .eval(&expr, 0)?;
        let Literal::Int(number) = literal else {
            bail!("{RPC}: diagnostic {name} must be an integer");
        };
        let number = number
            .to_string()
            .parse::<usize>()
            .context("diagnostic limit is outside usize")?;
        ensure!(
            number > 0 && number <= MAX_SOURCE_BYTES,
            "diagnostic limit is outside extraction bounds"
        );
        rows.insert(name.to_string(),json!({"line":line(&source,span.start().to_usize()),"end_line":line(&source,span.end().to_usize()),"value":number,"expression":"inline diagnostic bound"}));
    }
    let mut calls = Vec::new();
    super::effects::all_expressions(&suite, &mut |expression| {
        if let ast::Expr::Call(call) = expression
            && expression_path(&call.func).as_deref() == Some("_safe_diagnostic")
        {
            calls.push(call.clone());
        }
    })?;
    for call in calls {
        ensure!(
            call_path(&call.func).as_deref() == Some("_safe_diagnostic")
                && call.args.len() == 1
                && call.keywords.len() == 1,
            "{RPC}:{}: changed diagnostic call form",
            line(&source, call.range.start().to_usize())
        );
        let keyword = &call.keywords[0];
        ensure!(
            keyword
                .arg
                .as_ref()
                .is_some_and(|name| name.as_str() == "limit"),
            "diagnostic call has no exact limit keyword"
        );
        let budget = Cell::new(MAX_ALLOCATION_UNITS);
        let number = super::Evaluator {
            bindings: &super::Bindings::new(),
            budget: &budget,
        }
        .eval(&keyword.value, 0)?
        .json()?;
        let name = if number == rows["POLICY_METADATA_LIMIT"]["value"] {
            "POLICY_METADATA_LIMIT"
        } else if number == rows["POLICY_DETAIL_LIMIT"]["value"] {
            "POLICY_DETAIL_LIMIT"
        } else {
            bail!(
                "{RPC}:{}: diagnostic use differs from extracted metadata/detail bounds",
                line(&source, call.range.start().to_usize())
            );
        };
        let uses = rows
            .get_mut(name)
            .context("diagnostic row missing")?
            .as_object_mut()
            .context("diagnostic row is not an object")?
            .entry("uses")
            .or_insert_with(|| json!([]));
        uses.as_array_mut().context("diagnostic uses are not an array")?.push(json!({"line":line(&source,call.range.start().to_usize()),"end_line":line(&source,call.range.end().to_usize())}));
    }
    document["values"]
        .as_object_mut()
        .context("RPC values missing")?
        .extend(rows);
    Ok(())
}

fn value<'a>(document: &'a Value, name: &str) -> Result<&'a Value> {
    document
        .get("values")
        .and_then(|values| values.get(name))
        .and_then(|value| value.get("value"))
        .with_context(|| format!("missing extracted table {name}"))
}

fn header(document: &Value) -> Result<String> {
    let path = document["provenance"]["path"]
        .as_str()
        .context("missing source path")?;
    let commit = document["provenance"]["commit"]
        .as_str()
        .context("missing source commit")?;
    Ok(format!(
        "// Derived from RaoFoundation/subtensor {path} @ {commit} (Apache-2.0); modified by rtao.\n// @generated by cargo xtask extract. Do not edit.\n"
    ))
}

fn rust_string(value: &str) -> String {
    format!("{value:?}")
}

fn string_table(document: &Value, name: &str) -> Result<Vec<(String, Value)>> {
    value(document, name)?["dict"]
        .as_array()
        .context("table is not a dictionary")?
        .iter()
        .map(|pair| {
            Ok((
                pair[0]
                    .as_str()
                    .context("table key is not a string")?
                    .into(),
                pair[1].clone(),
            ))
        })
        .collect()
}

fn render_settings(document: &Value) -> Result<Vec<u8>> {
    let mut out = header(document)?;
    out.push_str(
        "//! Network presets and chain constants extracted from pinned Python settings.\n\n",
    );
    out.push_str("/// An endpoint literal or a runtime environment override.\n#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct Endpoint {\n    /// Name of the runtime environment variable, when present.\n    pub env: Option<&'static str>,\n    /// Endpoint used when the override is absent.\n    pub default: &'static str,\n    /// Whether an empty environment value uses the default.\n    pub empty_is_unset: bool,\n}\n\nimpl Endpoint {\n    /// Resolve the endpoint against a supplied environment reader.\n    pub fn resolve(self, read: impl FnOnce(&str) -> Option<String>) -> String {\n        self.env.and_then(read).filter(|value| !self.empty_is_unset || !value.is_empty()).unwrap_or_else(|| self.default.to_owned())\n    }\n}\n\n");
    for name in SETTINGS_NAMES {
        let value = value(document, name)?;
        if value.is_object() {
            continue;
        }
        let (ty, rendered) = if let Some(text) = value.as_str() {
            ("&str", rust_string(text))
        } else if name == &"BLOCKTIME" {
            ("f64", value.to_string())
        } else {
            ("u64", value.to_string())
        };
        out.push_str(&format!("/// Python `{name}`.\n#[doc(alias = \"bittensor.settings.{name}\")]\npub const {name}: {ty} = {rendered};\n\n"));
    }
    out.push_str("/// Network labels and endpoint defaults.\n#[doc(alias = \"bittensor.settings.NETWORKS\")]\npub const NETWORKS: &[(&str, Endpoint)] = &[\n");
    for (network, endpoint) in string_table(document, "NETWORKS")? {
        let (env, default, empty) = if let Some(endpoint) = endpoint.as_str() {
            ("None".into(), endpoint.to_owned(), false)
        } else {
            (
                format!(
                    "Some({})",
                    rust_string(endpoint["env"].as_str().context("endpoint env missing")?)
                ),
                endpoint["default"]
                    .as_str()
                    .context("endpoint default missing")?
                    .into(),
                endpoint["empty_is_unset"]
                    .as_bool()
                    .context("endpoint empty rule missing")?,
            )
        };
        out.push_str(&format!(
            "    ({}, Endpoint {{ env: {env}, default: {}, empty_is_unset: {empty} }}),\n",
            rust_string(&network),
            rust_string(&default)
        ));
    }
    out.push_str("];\n\n");
    for name in ["FALLBACK_ENDPOINTS", "ARCHIVE_ENDPOINTS"] {
        out.push_str(&format!("/// Python `{name}` endpoint pools.\n#[doc(alias = \"bittensor.settings.{name}\")]\npub const {name}: &[(&str, &[&str])] = &[\n"));
        for (network, endpoints) in string_table(document, name)? {
            let endpoints = endpoints
                .as_array()
                .context("endpoint pool is not a list")?
                .iter()
                .map(|endpoint| {
                    endpoint
                        .as_str()
                        .map(rust_string)
                        .context("endpoint is not a string")
                })
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            out.push_str(&format!(
                "    ({}, &[{endpoints}]),\n",
                rust_string(&network)
            ));
        }
        out.push_str("];\n\n");
    }
    for name in [
        "EXPLORER_EXTRINSIC_URLS",
        "EXPLORER_BLOCK_URLS",
        "EXPLORER_SUBNET_URLS",
    ] {
        out.push_str(&format!("/// Python `{name}` URL templates.\n#[doc(alias = \"bittensor.settings.{name}\")]\npub const {name}: &[(&str, &str)] = &[\n"));
        for (network, template) in string_table(document, name)? {
            out.push_str(&format!(
                "    ({}, {}),\n",
                rust_string(&network),
                rust_string(template.as_str().context("URL template is not a string")?)
            ));
        }
        out.push_str("];\n\n");
    }
    out.push_str("/// Python account explorer templates, indexed by network and account kind.\n#[doc(alias = \"bittensor.settings.EXPLORER_ACCOUNT_URLS\")]\npub const EXPLORER_ACCOUNT_URLS: &[(&str, &[(&str, &str)])] = &[\n");
    for (network, templates) in string_table(document, "EXPLORER_ACCOUNT_URLS")? {
        let templates = templates["dict"]
            .as_array()
            .context("account templates are not a dictionary")?
            .iter()
            .map(|pair| {
                Ok(format!(
                    "({}, {})",
                    rust_string(pair[0].as_str().context("account kind is not a string")?),
                    rust_string(pair[1].as_str().context("account URL is not a string")?)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        out.push_str(&format!(
            "    ({}, &[{templates}]),\n",
            rust_string(&network)
        ));
    }
    out.push_str("];\n");
    Ok(out.into_bytes())
}

fn render_rpc(document: &Value) -> Result<Vec<u8>> {
    let mut out = header(document)?;
    out.push_str("//! Session defaults and policy literals extracted from pinned Python RPC.\n\n");
    for (python, rust, ty) in [
        ("POLICY_METADATA_LIMIT", "POLICY_METADATA_LIMIT", "usize"),
        ("POLICY_DETAIL_LIMIT", "POLICY_DETAIL_LIMIT", "usize"),
        ("POLICY_BODY_LIMIT", "POLICY_BODY_LIMIT", "usize"),
        ("_STATE_DISCARDED_NEEDLE", "STATE_DISCARDED_NEEDLE", "&str"),
        ("_HANDSHAKE_TIMEOUT", "HANDSHAKE_TIMEOUT", "f64"),
        (
            "_SUBSCRIPTION_QUEUE_MAXSIZE",
            "SUBSCRIPTION_QUEUE_MAXSIZE",
            "usize",
        ),
        ("_CLOSED_IDS_REMEMBERED", "CLOSED_IDS_REMEMBERED", "usize"),
        ("_MAX_EARLY_BUFFERS", "MAX_EARLY_BUFFERS", "usize"),
        (
            "RpcSession.__init__.retry_forever",
            "DEFAULT_RETRY_FOREVER",
            "bool",
        ),
        (
            "RpcSession.__init__.max_retries",
            "DEFAULT_MAX_RETRIES",
            "usize",
        ),
        (
            "RpcSession.__init__.response_timeout",
            "DEFAULT_RESPONSE_TIMEOUT",
            "f64",
        ),
    ] {
        let value = value(document, python)?;
        let rendered = value
            .as_str()
            .map_or_else(|| value.to_string(), rust_string);
        out.push_str(&format!("/// Python `{python}`.\n#[doc(alias = \"bittensor._transport.rpc.{python}\")]\npub const {rust}: {ty} = {rendered};\n\n"));
    }
    for (python, rust, ty) in [
        ("_POLICY_RPC_ERROR_CODES", "POLICY_RPC_ERROR_CODES", "i64"),
        (
            "_POLICY_RPC_METADATA_KEYS",
            "POLICY_RPC_METADATA_KEYS",
            "&str",
        ),
        ("_NON_IDEMPOTENT_METHODS", "NON_IDEMPOTENT_METHODS", "&str"),
    ] {
        let values = value(document, python)?["frozenset"]
            .as_array()
            .context("RPC constant is not a frozenset")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), rust_string)
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("/// Python `{python}`.\n#[doc(alias = \"bittensor._transport.rpc.{python}\")]\npub const {rust}: &[{ty}] = &[{values}];\n\n"));
    }
    out.push_str("/// Exact RPC policy messages, indexed by JSON-RPC error code.\n#[doc(alias = \"bittensor._transport.rpc._POLICY_RPC_ERROR_MESSAGES\")]\npub const POLICY_RPC_ERROR_MESSAGES: &[(i64, &[&str])] = &[\n");
    for entry in value(document, "_POLICY_RPC_ERROR_MESSAGES")?["dict"]
        .as_array()
        .context("policy messages are not a dictionary")?
    {
        let messages = entry[1]["frozenset"]
            .as_array()
            .context("policy messages are not a frozenset")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(rust_string)
                    .context("policy message is not a string")
            })
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        out.push_str(&format!("    ({}, &[{messages}]),\n", entry[0]));
    }
    out.push_str("];\n");
    Ok(out.into_bytes())
}

fn pretty(document: &Value) -> Result<Vec<u8>> {
    let mut canonical = document.clone();
    canonical.sort_all_objects();
    let mut bytes = serde_json::to_vec_pretty(&canonical)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn rustfmt(source: Vec<u8>) -> Result<Vec<u8>> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("run pinned rustfmt for extracted Rust tables")?;
    child
        .stdin
        .take()
        .context("rustfmt stdin missing")?
        .write_all(&source)?;
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "rustfmt rejected extracted Rust: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn outputs(root: &Path) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
    let mut resolver = Resolver::new(root)?;
    let mut settings = document(&mut resolver, &pins, SETTINGS, SETTINGS_NAMES)?;
    settings.sort_all_objects();
    let mut rpc = document(&mut resolver, &pins, RPC, RPC_NAMES)?;
    rpc_diagnostic_limits(&resolver, &mut rpc)?;
    rpc.sort_all_objects();
    let mut tables = serde_json::Map::new();
    for (path, names) in NAMED_TABLES {
        let mut resolver = Resolver::new(root)?;
        tables.insert((*path).into(), document(&mut resolver, &pins, path, names)?);
    }
    let mut resolver = Resolver::new(root)?;
    let intent_paths = resolver
        .tree
        .keys()
        .filter(|path| path.starts_with("sdk/python/bittensor/intents/") && path.ends_with(".py"))
        .cloned()
        .collect::<Vec<_>>();
    for path in intent_paths {
        resolver.module(&path)?;
        let classes = intent_classes(&resolver.source(&path)?, &path)?;
        let names = resolver.modules[&path]
            .locations
            .keys()
            .filter(|name| {
                name.contains('.')
                    && name
                        .split('.')
                        .next()
                        .is_some_and(|class| classes.contains(class))
                    && name.matches('.').count() == 1
            })
            .cloned()
            .collect::<Vec<_>>();
        if !names.is_empty() {
            let names = names.iter().map(String::as_str).collect::<Vec<_>>();
            let defaults = document(&mut resolver, &pins, &path, &names)?;
            if let Some(existing) = tables.get_mut(&path) {
                existing["values"]
                    .as_object_mut()
                    .context("named values are not an object")?
                    .extend(
                        defaults["values"]
                            .as_object()
                            .context("default values are not an object")?
                            .clone(),
                    );
            } else {
                tables.insert(path.clone(), defaults);
            }
        }
    }
    let mut tables = Value::Object(tables);
    // JSON object order describes the generated schema. Python mapping order
    // remains in Literal::Dict entry arrays and must never be sorted.
    tables.sort_all_objects();
    let mut generated = catalog::outputs(root, &pins, &tables)?;
    generated.extend(hyperparams::outputs(&tables, &settings)?);
    generated.extend(owner_map::outputs(root, &pins, &tables)?);
    generated.extend(chain::outputs(root, &pins, &tables)?);
    generated.extend(env_vars::outputs(root, &pins, &tables)?);
    generated.extend(read_params::outputs(root, &pins, &tables)?);
    let read_document = read_params::document(root, &pins, &tables)?;
    generated.extend(read_hits::outputs(root, &pins, &tables, &read_document)?);
    generated.extend(specs::outputs(root, &pins, &tables, &read_document)?);
    let mut py = render_settings(&settings)?;
    py.extend_from_slice(py_tables::render(&tables)?.as_bytes());
    generated.extend(vec![
        (
            "vendor/upstream/extracted/settings.json",
            pretty(&settings)?,
        ),
        ("vendor/upstream/extracted/rpc.json", pretty(&rpc)?),
        ("crates/rtao-catalog/src/py.rs", rustfmt(py)?),
        (
            "crates/rtao-rpc/src/generated/py_rpc.rs",
            rustfmt(render_rpc(&rpc)?)?,
        ),
        (
            "vendor/upstream/extracted/named_tables.json",
            pretty(&json!({"format":1,"tables":tables}))?,
        ),
    ]);
    Ok(generated)
}

pub(super) fn extract_at(root: &Path, check: bool) -> Result<()> {
    for (relative, expected) in outputs(root)? {
        let path = root.join(relative);
        if check {
            ensure!(
                fs::read(&path)
                    .with_context(|| format!("read {relative}; run cargo xtask extract"))?
                    == expected,
                "{relative} is stale or modified; run cargo xtask extract"
            );
        } else if fs::read(&path).ok().as_deref() != Some(&expected) {
            fs::create_dir_all(path.parent().context("extraction output has no parent")?)?;
            fs::write(path, expected)?;
        }
    }
    Ok(())
}

pub(super) fn public_exports(root: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    let mut resolver = Resolver::new(root)?;
    let paths = resolver
        .tree
        .keys()
        .filter(|path| {
            path.starts_with("sdk/python/bittensor/")
                && path.ends_with(".py")
                && !path.starts_with("sdk/python/bittensor/cli/")
                && !path.starts_with("sdk/python/bittensor/_transport/")
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut exports = BTreeMap::new();
    for path in paths {
        let source = resolver.source(&path)?;
        let suite =
            ast::Suite::parse(&source, &path).map_err(|error| anyhow::anyhow!("{error}"))?;
        if !suite.iter().any(|stmt| {
            super::effects::conditional_names(std::slice::from_ref(stmt)).contains("__all__")
        }) {
            continue;
        }
        let names = if path == "sdk/python/bittensor/__init__.py" {
            root_public_exports(root, &suite)?
        } else {
            let values = resolver.values(&path, &["__all__"])?;
            values["__all__"]["value"]
                .as_array()
                .context("public __all__ must resolve to a list")?
                .iter()
                .map(|name| {
                    name.as_str()
                        .map(str::to_owned)
                        .context("__all__ names must be strings")
                })
                .collect::<Result<Vec<_>>>()?
        };
        ensure!(
            names.iter().collect::<BTreeSet<_>>().len() == names.len(),
            "duplicate public exports in {path}"
        );
        exports.insert(path, names);
    }
    ensure!(!exports.is_empty(), "no public Python export lists found");
    Ok(exports)
}

fn root_public_exports(root: &Path, suite: &[ast::Stmt]) -> Result<Vec<String>> {
    let bindings = |name: &str| {
        suite
            .iter()
            .filter(|stmt| {
                super::effects::conditional_names(std::slice::from_ref(*stmt)).contains(name)
            })
            .collect::<Vec<_>>()
    };
    let imports = bindings("_INTENT_REGISTRY");
    ensure!(
        imports.len() == 1
            && matches!(imports[0],ast::Stmt::ImportFrom(import) if import.module.as_ref().is_some_and(|module|module.as_str()=="intents") && import.level.is_some_and(|level|level.to_usize()==1) && import.names.iter().any(|alias|alias.name.as_str()=="REGISTRY" && alias.asname.as_ref().is_some_and(|name|name.as_str()=="_INTENT_REGISTRY"))),
        "root intent export registry import changed"
    );
    let exports = bindings("_INTENT_EXPORTS");
    ensure!(
        exports.len() == 1,
        "root intent export derivation has duplicate/conditional bindings"
    );
    let ast::Stmt::Assign(assign) = exports[0] else {
        bail!("root intent export derivation must be an assignment")
    };
    let ast::Expr::DictComp(comprehension) = &*assign.value else {
        bail!("root intent exports must derive directly from the registry")
    };
    ensure!(
        comprehension.generators.len() == 1
            && call_path(&comprehension.key).as_deref() == Some("cls.__name__")
            && call_path(&comprehension.value).as_deref() == Some("cls"),
        "root intent export comprehension changed"
    );
    let generator = &comprehension.generators[0];
    ensure!(
        call_path(&generator.target).as_deref() == Some("cls")
            && generator.ifs.is_empty()
            && !generator.is_async
            && matches!(&generator.iter,ast::Expr::Call(call) if call_path(&call.func).as_deref()==Some("_INTENT_REGISTRY.values") && call.args.is_empty() && call.keywords.is_empty()),
        "root intent exports may not filter or transform the registry"
    );
    let declarations = bindings("__all__");
    ensure!(
        declarations.len() == 1,
        "root __all__ has duplicate/conditional bindings"
    );
    let ast::Stmt::Assign(assign) = declarations[0] else {
        bail!("root __all__ must be an assignment")
    };
    let ast::Expr::List(list) = &*assign.value else {
        bail!("root __all__ must remain a literal list plus registered classes")
    };
    let mut names = Vec::new();
    let mut starred = 0;
    for value in &list.elts {
        match value {
            ast::Expr::Constant(ast::ExprConstant {
                value: ast::Constant::Str(value),
                ..
            }) => names.push(value.clone()),
            ast::Expr::Starred(value) => {
                ensure!(
                    matches!(&*value.value,ast::Expr::Call(call) if call_path(&call.func).as_deref()==Some("sorted") && call.args.len()==1 && call.keywords.is_empty() && call_path(&call.args[0]).as_deref()==Some("_INTENT_EXPORTS")),
                    "root starred export expression changed"
                );
                starred += 1;
            }
            _ => bail!("root export name must be literal or checked registered intent classes"),
        }
    }
    ensure!(
        starred == 1 && bindings("sorted").is_empty(),
        "root sorted export derivation changed or builtin shadowed"
    );
    let catalog_bytes = fs::read(root.join("vendor/upstream/catalog/intents.json"))?;
    let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
    let pin = &pins["artifacts"]["website/apps/bittensor-website/public/catalog/intents.json"];
    ensure!(
        pin["to"].as_str() == Some("vendor/upstream/catalog/intents.json")
            && pin["sha256"].as_str()
                == Some(format!("{:x}", Sha256::digest(&catalog_bytes)).as_str()),
        "root derived exports require the authenticated intent catalog"
    );
    let catalog: Value = serde_json::from_slice(&catalog_bytes)?;
    let mut classes = catalog
        .as_array()
        .context("intent catalog missing")?
        .iter()
        .map(|row| {
            row["python_class"]
                .as_str()
                .map(str::to_owned)
                .context("intent class missing")
        })
        .collect::<Result<Vec<_>>>()?;
    classes.sort();
    names.extend(classes);
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_schema_keeps_python_dictionary_and_list_order() -> Result<()> {
        let first: Value = serde_json::from_str(
            r#"{"z":{"b":2,"a":1},"a":{"dict":[["z",1],["a",2]],"list":[3,1,2]}}"#,
        )?;
        let second: Value = serde_json::from_str(
            r#"{"a":{"list":[3,1,2],"dict":[["z",1],["a",2]]},"z":{"a":1,"b":2}}"#,
        )?;
        let canonical = pretty(&first)?;
        assert_eq!(canonical, pretty(&second)?);
        let round_trip: Value = serde_json::from_slice(&canonical)?;
        assert_eq!(round_trip["a"]["dict"], json!([["z", 1], ["a", 2]]));
        assert_eq!(round_trip["a"]["list"], json!([3, 1, 2]));
        Ok(())
    }

    #[test]
    fn public_exports_authenticate_lists_and_check_root_registry_derivation() -> Result<()> {
        let root = root()?;
        let exports = public_exports(&root)?;
        assert_eq!(exports.len(), 15);
        let root_names = exports
            .get("sdk/python/bittensor/__init__.py")
            .context("root exports absent")?;
        for spec in &rtao_catalog::INTENTS {
            assert!(root_names.iter().any(|name| name == spec.python_class));
        }
        let source = Resolver::new(&root)?.source("sdk/python/bittensor/__init__.py")?;
        for changed in [
            source.replace("*sorted(_INTENT_EXPORTS)", "*list(_INTENT_EXPORTS)"),
            source.replace("cls.__name__: cls", "cls.__name__.lower(): cls"),
            source.replace(
                "_INTENT_REGISTRY.values()}",
                "_INTENT_REGISTRY.values() if cls}",
            ),
        ] {
            let suite = ast::Suite::parse(&changed, "exports-fixture.py")
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            assert!(root_public_exports(&root, &suite).is_err());
        }
        Ok(())
    }

    fn root() -> Result<PathBuf> {
        Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace")?
            .to_owned())
    }

    fn fixture(source: &str, name: &str) -> Result<Value> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        resolver.sources.insert("fixture.py".into(), source.into());
        Ok(resolver.values("fixture.py", &[name])?[name]["value"].clone())
    }

    #[test]
    fn charges_alias_clones_and_rechecks_dictionary_mutations() -> Result<()> {
        let source = format!(
            "TABLE = 'x' * 1000000\n{}",
            (0..12)
                .map(|n| format!("A{n} = TABLE\n"))
                .collect::<String>()
        );
        let error = fixture(&source, "A11")
            .err()
            .context("alias allocation must fail")?;
        assert!(format!("{error:#}").contains("allocation limit"));
        assert!(fixture("TABLE = {}\nTABLE[[]] = 1", "TABLE").is_err());
        let entries = (0..MAX_CONTAINER_ITEMS)
            .map(|n| format!("{n}:0"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            fixture(
                &format!("TABLE = {{{entries}}}\nTABLE[{MAX_CONTAINER_ITEMS}] = 0"),
                "TABLE"
            )
            .is_err()
        );
        assert_eq!(
            fixture("TABLE = {'a':1}\nTABLE['b']=2", "TABLE")?,
            json!({"dict":[["a",1],["b",2]]})
        );
        Ok(())
    }

    #[test]
    fn refuses_mutable_aliases_unsupported_effects_and_conditional_rebindings() -> Result<()> {
        for (source, name) in [
            ("TABLE={'a':1}\nWRAPPED=[TABLE]\nTABLE['a']=2", "WRAPPED"),
            ("A=B={}\nA['a']=1", "B"),
            (
                "TABLE={'a':1}\nclass C:\n    ALIAS=TABLE\nTABLE['a']=2",
                "C.ALIAS",
            ),
            (
                "TABLE={'a':1}\nclass C:\n    FIELD=field(default_factory=lambda: TABLE)\nTABLE['a']=2",
                "C.FIELD",
            ),
            ("TABLE=[1]\nOTHER=TABLE.append(2)", "TABLE"),
            ("TABLE=[1]\nmutate(TABLE)", "TABLE"),
            (
                "import unknown\nTABLE=[1]\nOTHER=unknown.mutate(TABLE)",
                "TABLE",
            ),
            (
                "TABLE=[1]\ndef list(value):\n    value.append(2)\nOTHER=list(TABLE)",
                "TABLE",
            ),
            ("TABLE=[1]\ndef f(arg=TABLE.append(2)):\n    pass", "TABLE"),
            ("TABLE=[1]\n@modify(TABLE)\ndef f():\n    pass", "TABLE"),
            (
                "class C:\n    TABLE={}\n    TABLE.update({'a':1})",
                "C.TABLE",
            ),
            ("TABLE=1\nif flag:\n    from unknown import TABLE", "TABLE"),
            ("TABLE=1\nif flag:\n    def TABLE():\n        pass", "TABLE"),
            ("TABLE=1\nwith unknown as TABLE:\n    pass", "TABLE"),
            (
                "class C:\n    TABLE=1\n    if flag:\n        TABLE=2",
                "C.TABLE",
            ),
        ] {
            assert!(fixture(source, name).is_err(), "{source}");
        }
        assert_eq!(
            fixture("TABLE=[1]\ndef f():\n    TABLE.append(2)", "TABLE")?,
            json!([1])
        );
        Ok(())
    }

    #[test]
    fn derived_wraps_rejects_changed_ast_and_keeps_insertion_order() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        resolver.module("sdk/python/bittensor/intents/hyperparameters.py")?;
        let bindings =
            &resolver.modules["sdk/python/bittensor/intents/hyperparameters.py"].bindings;
        let source = "tuple(dict.fromkeys(('AdminUtils', method) for method, _ in OWNER_HYPERPARAMETERS.values()))";
        let parse = |source: &str| {
            ast::Expr::parse(source, "derived.py").map_err(|error| anyhow::anyhow!("{error}"))
        };
        let Binding::Value(wraps) = derive_hyperparam_wraps(&parse(source)?, bindings)? else {
            bail!("derived wraps missing");
        };
        let wraps = wraps.json()?;
        assert_eq!(wraps["tuple"].as_array().context("wraps tuple")?.len(), 33);
        assert_eq!(
            wraps["tuple"][0],
            json!({"tuple":["AdminUtils","sudo_set_epoch_consensus"]})
        );
        for changed in [
            source.replace("'AdminUtils'", "'Other'"),
            source.replace("('AdminUtils', method)", "('AdminUtils', method, _)"),
            source.replace(".values()", "['x'].values()"),
            source.replace("tuple(", "tuple[0]("),
            source.replace("dict.fromkeys(", "dict.fromkeys[0]("),
            source.replace(".values()))", ".values() if method))"),
        ] {
            assert!(
                derive_hyperparam_wraps(&parse(&changed)?, bindings).is_err(),
                "{changed}"
            );
        }
        Ok(())
    }

    #[test]
    fn descriptor_payload_and_inline_rpc_limits_are_source_checked() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let source = resolver.source("sdk/python/bittensor/_generated/storage.py")?;
        resolver.sources.insert(
            "sdk/python/bittensor/_generated/storage.py".into(),
            source.replace(
                "Rho = Item('SubtensorModule', 'Rho', 'u16')",
                "Rho = Item('SubtensorModule', 'Kappa', 'u16')",
            ),
        );
        assert!(
            resolver
                .descriptor("sdk/python/bittensor/_generated/storage.py.SubtensorModule.Rho")
                .is_err()
        );
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let mut rpc = document(&mut resolver, &pins, RPC, RPC_NAMES)?;
        rpc_diagnostic_limits(&resolver, &mut rpc)?;
        assert_eq!(value(&rpc, "POLICY_METADATA_LIMIT")?, &json!(200));
        assert_eq!(
            rpc["values"]["POLICY_METADATA_LIMIT"]["uses"]
                .as_array()
                .context("header uses")?
                .len(),
            4
        );
        let source = resolver.source(RPC)?;
        resolver.sources.insert(
            RPC.into(),
            source.replace("limit=200) if retry_after", "limit=201) if retry_after"),
        );
        assert!(rpc_diagnostic_limits(&resolver, &mut rpc).is_err());
        Ok(())
    }

    #[test]
    fn extract_check_covers_named_tables() -> Result<()> {
        let root = root()?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let mut resolver = Resolver::new(&root)?;
        let settings = document(&mut resolver, &pins, SETTINGS, SETTINGS_NAMES)?;
        let mut rpc = document(&mut resolver, &pins, RPC, RPC_NAMES)?;
        rpc_diagnostic_limits(&resolver, &mut rpc)?;
        assert_eq!(
            settings["values"]
                .as_object()
                .context("settings values")?
                .len(),
            SETTINGS_NAMES.len()
        );
        assert_eq!(
            rpc["values"].as_object().context("RPC values")?.len(),
            RPC_NAMES.len() + 3
        );
        assert_eq!(value(&settings, "RAO_PER_TAO")?, &json!(1_000_000_000));
        assert_eq!(value(&rpc, "RpcSession.__init__.max_retries")?, &json!(5));
        assert_eq!(
            value(&rpc, "RpcSession.__init__.response_timeout")?,
            &json!(60.0)
        );
        assert_eq!(
            value(&settings, "EXPLORER_BLOCK_URLS")?["dict"]
                .as_array()
                .context("block explorers")?
                .len(),
            2
        );
        extract_at(&root, true)?;
        let aggregate: Value = serde_json::from_slice(&fs::read(
            root.join("vendor/upstream/extracted/named_tables.json"),
        )?)?;
        for (path, names) in NAMED_TABLES {
            let extracted = aggregate["tables"][path]["values"]
                .as_object()
                .with_context(|| format!("missing named-table source {path}"))?;
            assert!(extracted.len() >= names.len(), "{path}");
            for name in *names {
                assert!(extracted.contains_key(*name), "{path}: missing {name}");
            }
        }
        let intents: Value = serde_json::from_slice(&fs::read(
            root.join("vendor/upstream/catalog/intents.json"),
        )?)?;
        for intent in intents.as_array().context("intent catalog is not a list")? {
            let class = intent["python_class"]
                .as_str()
                .context("intent class missing")?;
            let key = format!("{class}.op");
            let found = aggregate["tables"]
                .as_object()
                .context("named tables missing")?
                .values()
                .filter_map(|table| table["values"].get(&key))
                .collect::<Vec<_>>();
            assert_eq!(
                found.len(),
                1,
                "{class}: every catalog intent must have exactly one extracted op"
            );
            assert_eq!(found[0]["value"], intent["name"], "{class}");
        }
        Ok(())
    }

    #[test]
    fn home_path_strings_preserve_representation_and_refuse_path_arithmetic() -> Result<()> {
        assert_eq!(
            fixture(
                "from pathlib import Path\nTABLE = str(Path.home() / '.bittensor' / 'wallets')",
                "TABLE"
            )?,
            json!({"home_relative":[".bittensor","wallets"],"path_string":true})
        );
        for source in [
            "from pathlib import Path\nTABLE = str(Path.home()) / 'wallets'",
            "from pathlib import Path\nTABLE = str(str(Path.home()))",
            "TABLE = str('wallets')",
            "from pathlib import Path\nTABLE = str(Path.home(), 'utf8')",
        ] {
            assert!(fixture(source, "TABLE").is_err(), "{source}");
        }
        Ok(())
    }

    #[test]
    fn resolves_builtin_tags_home_paths_imported_names_and_descriptors() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let config = resolver.values(
            "sdk/python/bittensor/config.py",
            &[
                "DEFAULT_CONFIG_PATH",
                "SETTABLE",
                "TOKEN_SYMBOLS_TTL_SECONDS",
            ],
        )?;
        assert_eq!(
            config["DEFAULT_CONFIG_PATH"]["value"],
            json!({"home_relative":[".bittensor","btcli.json"]})
        );
        assert_eq!(
            config["SETTABLE"]["value"]["dict"][0],
            json!(["network",{"type":"str"}])
        );
        assert_eq!(config["TOKEN_SYMBOLS_TTL_SECONDS"]["value"], json!(604800));
        let hyper = resolver.values(
            "sdk/python/bittensor/hyperparams.py",
            &["RATIO_TYPE_DENOMINATORS", "HYPERPARAMS", "STORAGE_ITEMS"],
        )?;
        assert_eq!(
            hyper["RATIO_TYPE_DENOMINATORS"]["value"]["dict"][0],
            json!(["PerU16", 65535])
        );
        assert_eq!(
            hyper["STORAGE_ITEMS"]["value"]["dict"][0][1],
            json!({"pallet":"SubtensorModule","item":"Rho","value_type_ident":"u16","source_value_type_ident":"u16"})
        );
        assert_eq!(
            hyper["HYPERPARAMS"]["value"]["dict"][0][1]["fields"]["maximum"],
            json!(1)
        );
        Ok(())
    }

    #[test]
    fn accepts_factory_lambda_without_execution() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let source = "field(default_factory=lambda: [0, 2**80])";
        let expr =
            ast::Expr::parse(source, "factory.py").map_err(|error| anyhow::anyhow!("{error}"))?;
        let mut bindings = Bindings::new();
        bindings.insert("field".into(), Binding::Symbol("dataclasses.field".into()));
        assert_eq!(
            resolver
                .eval(&expr, &bindings, "factory.py", 0)?
                .json()?
                .to_string(),
            "[0,1208925819614629174706176]"
        );
        for source in [
            "field(default_factory=lambda arg: [arg])",
            "field(default_factory=lambda: open('/tmp/forbidden'))",
            "field(default_factory=unknown)",
            "Path.home('/tmp')",
        ] {
            let expr = ast::Expr::parse(source, "factory.py")
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            assert!(
                resolver.eval(&expr, &bindings, "factory.py", 0).is_err(),
                "{source}"
            );
        }
        Ok(())
    }

    #[test]
    fn environment_defaults_preserve_empty_semantics_and_never_read_host() -> Result<()> {
        let root = root()?;
        let mut resolver = Resolver::new(&root)?;
        let mut bindings = Bindings::new();
        bindings.insert("os".into(), Binding::Module("os".into()));
        for (source, expected) in [
            (
                "os.getenv('RTAO_TEST_ENV') or 'fallback'",
                json!({"env":"RTAO_TEST_ENV","default":"fallback","empty_is_unset":true}),
            ),
            (
                "os.environ.get('RTAO_TEST_ENV', 'fallback')",
                json!({"env":"RTAO_TEST_ENV","default":"fallback","empty_is_unset":false}),
            ),
        ] {
            let expr = ast::Expr::parse(source, "environment.py")
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            assert_eq!(
                resolver
                    .eval(&expr, &bindings, "environment.py", 0)?
                    .json()?,
                expected
            );
        }
        Ok(())
    }
}
