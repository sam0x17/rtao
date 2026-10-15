// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/main.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Source-only inventory of pinned Typer registrations. No Python is imported.
// This records literal option spellings and positional names, not Click's
// resolved types, defaults or environment precedence. Generated registry
// interfaces remain covered by their catalog/schema tests.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const base = 'vendor/upstream/src/sdk/python/bittensor/cli/';
const sourceNames = [
  'main.py', 'globals.py', 'call.py', 'stake_exit.py', 'commands/wallet.py',
  'commands/stake.py', 'commands/root.py', 'commands/root_move.py',
  'commands/root_swap.py', 'commands/subnets.py', 'commands/sudo.py',
  'commands/lock.py', 'commands/addresses.py', 'commands/proxy.py',
  'commands/multisig.py', 'commands/config.py', 'commands/misc.py',
  'commands/weights.py', 'commands/axon.py', 'commands/crowd.py',
  'commands/collateral.py', 'commands/upgrade.py', 'commands/timelock.py',
  'commands/extension.py', 'commands/evm/__init__.py', 'commands/evm/keys.py',
  'commands/evm/stake.py', 'commands/evm/address.py', 'commands/evm/money.py',
  'commands/evm/association.py', 'commands/evm/precompiles_cmd.py',
  'commands/evm/contracts_cmd.py', 'commands/evm/setup.py',
  'commands/evm/_shared.py',
];

function lex(source) {
  const tokens = [];
  for (let cursor = 0; cursor < source.length;) {
    const start = cursor;
    if (/\s/.test(source[cursor])) { cursor++; continue; }
    if (source[cursor] === '#') {
      while (cursor < source.length && source[cursor] !== '\n') cursor++;
      continue;
    }
    if (source[cursor] === '"' || source[cursor] === "'") {
      const quote = source[cursor];
      const triple = source.slice(cursor, cursor + 3) === quote.repeat(3);
      const terminator = quote.repeat(triple ? 3 : 1);
      cursor += terminator.length;
      let value = '';
      while (cursor < source.length && source.slice(cursor, cursor + terminator.length) !== terminator) {
        if (source[cursor] === '\\') cursor++;
        value += source[cursor++];
      }
      if (cursor >= source.length) throw new Error(`unterminated string at ${start}`);
      cursor += terminator.length;
      tokens.push({ value, kind: 'string', start, end: cursor });
    } else if (/[a-zA-Z_0-9]/.test(source[cursor])) {
      while (cursor < source.length && /[a-zA-Z_0-9]/.test(source[cursor])) cursor++;
      tokens.push({ value: source.slice(start, cursor), kind: 'name', start, end: cursor });
    } else {
      tokens.push({ value: source[cursor++], kind: 'punct', start, end: cursor });
    }
  }
  return tokens;
}

const closing = new Map([['(', ')'], ['[', ']'], ['{', '}']]);
function close(tokens, first) {
  const stack = [];
  for (let cursor = first; cursor < tokens.length; cursor++) {
    if (tokens[cursor].kind !== 'punct') continue;
    const value = tokens[cursor].value;
    if (closing.has(value)) stack.push(closing.get(value));
    else if ([')', ']', '}'].includes(value)) {
      if (stack.pop() !== value) throw new Error('mismatched source brackets');
      if (!stack.length) return cursor;
    }
  }
  throw new Error('unclosed source bracket');
}

function argumentsOf(tokens, first, last) {
  const result = [];
  let begin = first;
  for (let cursor = first; cursor < last; cursor++) {
    if (tokens[cursor].kind !== 'punct') continue;
    const value = tokens[cursor].value;
    if (closing.has(value)) cursor = close(tokens, cursor);
    else if (value === ',') {
      result.push(tokens.slice(begin, cursor));
      begin = cursor + 1;
    }
  }
  if (begin < last) result.push(tokens.slice(begin, last));
  return result.filter(arg => arg.length);
}

const sources = sourceNames.map(name => {
  const source = fs.readFileSync(path.join(root, base, name), 'utf8');
  return { name, source, tokens: lex(source) };
});

function signature(source, name) {
  const tokens = source.tokens;
  const first = tokens.findIndex((token, index) => token.value === 'def' && tokens[index + 1]?.value === name);
  if (first < 0) throw new Error(`missing function ${source.name}:${name}`);
  const start = first + 2;
  const end = close(tokens, start);
  return argumentsOf(tokens, start + 1, end);
}

function surface(parameters) {
  const options = new Set();
  const positionals = [];
  const unresolved = [];
  for (const param of parameters) {
    const name = param[0]?.value;
    if (!name || name === 'ctx' || name === '*') continue;
    const annotation = param.findIndex(token => token.value === '=');
    const fallback = annotation < 0 ? [] : param.slice(annotation + 1);
    const kind = fallback.findIndex((token, index) => token.value === 'typer' && fallback[index + 1]?.value === '.' && ['Option', 'Argument'].includes(fallback[index + 2]?.value));
    if (kind >= 0) {
      if (fallback[kind + 2].value === 'Argument') {
        positionals.push(name);
        continue;
      }
      const open = kind + 3;
      const args = argumentsOf(fallback, open + 1, close(fallback, open));
      let declared = false;
      for (const arg of args.slice(1)) {
        if (arg.length === 1 && arg[0].kind === 'string' && arg[0].value.startsWith('-')) {
          for (const spelling of arg[0].value.split('/')) options.add(spelling);
          declared = true;
        } else if (arg[0]?.value === 'address_cli_name' && arg[2]?.kind === 'string') {
          options.add('--' + arg[2].value.replace(/_ss58$/, '').replaceAll('_ss58', '').replaceAll('_', '-'));
          declared = true;
        }
      }
      if (!declared) options.add('--' + name.replaceAll('_', '-'));
    } else if (fallback.some(token => token.value === 'RPC_URL_OPTION')) options.add('--rpc-url');
    else if (fallback.some(token => token.value === '_proxy_type_option')) options.add('--proxy-type');
    else if (!fallback.length || fallback.length === 1) positionals.push(name);
    else unresolved.push(name);
  }
  return { options: [...options].sort(), positionals, unresolved };
}

function defaultPath(name) {
  if (name === 'main.py') return '';
  if (name.startsWith('commands/evm/')) return 'evm';
  const stem = path.basename(name, '.py');
  if (['weights', 'axon', 'collateral', 'timelock', 'crowd', 'extension', 'upgrade'].includes(stem)) return `misc ${stem}`;
  return ({ sudo: 'hparams', lock: 'conviction', addresses: 'addr' })[stem] ?? stem;
}

const rows = [];
for (const source of sources) {
  if (['globals.py', 'stake_exit.py', 'commands/root_move.py', 'commands/root_swap.py', 'commands/evm/_shared.py'].includes(source.name)) continue;
  const tokens = source.tokens;
  const containers = new Map([['app', defaultPath(source.name)]]);
  for (const [container, suffix] of [['keychain_app', 'keychain'], ['book_app', 'book'], ['child_app', 'child'], ['mechanisms_app', 'mechanisms']]) {
    containers.set(container, `${defaultPath(source.name)} ${suffix}`);
  }
  containers.set('key_app', 'evm key');
  containers.set('stake_app', 'evm stake');
  for (let index = 0; index < tokens.length - 4; index++) {
    if (!containers.has(tokens[index].value) || tokens[index + 1].value !== '.' || tokens[index + 2].value !== 'command' || tokens[index + 3].value !== '(') continue;
    const end = close(tokens, index + 3);
    const args = argumentsOf(tokens, index + 4, end);
    const explicitName = args[0]?.[0]?.kind === 'string' && args[0].length === 1 ? args[0][0].value : undefined;
    if (args[0]?.length === 1 && args[0][0].kind === 'name') continue; // registered in a literal tuple loop below
    const decorator = tokens[index - 1]?.value === '@';
    let functionName;
    let target = source;
    let generated;
    if (decorator) {
      const next = tokens.findIndex((token, cursor) => cursor > end && token.value === 'def');
      if (next < 0) throw new Error(`missing decorated function ${source.name}`);
      functionName = tokens[next + 1].value;
    } else if (tokens[end + 1]?.value === '(') {
      if (tokens[end + 2]?.value === 'intent_command') generated = tokens[end + 4].value;
      else if (tokens[end + 3]?.value === '.') {
        target = sources.find(item => item.name === `commands/${tokens[end + 2].value}.py`);
        functionName = tokens[end + 4].value;
      } else functionName = tokens[end + 2]?.value;
    }
    if (functionName === 'call_command') {
      target = sources.find(item => item.name === 'call.py');
      functionName = 'call';
    }
    const commandName = explicitName ?? functionName?.replaceAll('_', '-');
    if (!commandName) throw new Error(`unresolved registration ${source.name}:${index}`);
    const commandPath = [containers.get(tokens[index].value), commandName].filter(Boolean).join(' ');
    const detail = generated ? { generated_intent: generated, options: [], positionals: [], unresolved: [] } : surface(signature(target, functionName));
    rows.push({ path: commandPath, source: base + source.name, function: functionName ?? null, ...detail });
  }
}

const stake = sources.find(source => source.name === 'commands/stake.py').source;
const loopStart = stake.indexOf('for _alias, _op in (');
const loop = stake.slice(loopStart, stake.indexOf('mount_stake_exit_commands', loopStart));
for (const [, name, op] of loop.matchAll(/\("([^"]+)", "([^"]+)"\)/g)) {
  rows.push({ path: `stake ${name}`, source: base + 'commands/stake.py', function: null, generated_intent: op, options: [], positionals: [], unresolved: [] });
}
for (const name of ['unstake-all', 'unstake-all-alpha']) {
  rows.push({ path: `stake ${name}`, source: base + 'stake_exit.py', function: 'command', ...surface(signature(sources.find(source => source.name === 'stake_exit.py'), 'command')) });
}

const aliases = {
  w: 'wallet', wallets: 'wallet', st: 'stake', rt: 'root', s: 'subnets', subnet: 'subnets',
  su: 'hparams', sudo: 'hparams', lock: 'conviction', c: 'config', conf: 'config',
  wt: 'misc weights', weight: 'misc weights', weights: 'misc weights',
  cr: 'misc crowd', crowdloan: 'misc crowd', crowd: 'misc crowd', addresses: 'addr',
  extension: 'misc extension', timelock: 'misc timelock', axon: 'misc axon',
  collateral: 'misc collateral', upgrade: 'misc upgrade', utils: 'misc',
  'stake children': 'stake child', 'hparams mech': 'hparams mechanisms',
};
for (const [alias, canonical] of [['subnets hparams', 'subnets hyperparameters'], ['hparams buyback', 'hparams stake-burn']]) aliases[alias] = canonical;

const result = {
  schema: 1,
  provenance: 'Pinned Python source inspection; Python/Click was not executed. Literal registered commands, command-owned option spellings and positional names only. Resolved defaults/types/envvars and shared global tiers need separate evidence.',
  sources: sources.map(({ name, source }) => ({ path: base + name, sha256: crypto.createHash('sha256').update(source).digest('hex') })),
  aliases,
  commands: rows.sort((a, b) => a.path.localeCompare(b.path, 'en')),
};
if (rows.some(row => row.unresolved.length)) throw new Error(`unresolved parameters: ${JSON.stringify(rows.filter(row => row.unresolved.length))}`);
const snapshot = path.join(root, 'crates/rtao-cli/tests/fixtures/cli_source_tree.json');
const content = JSON.stringify(result, null, 2) + '\n';
if (process.argv.includes('--write')) fs.writeFileSync(snapshot, content);
else if (fs.readFileSync(snapshot, 'utf8') !== content) throw new Error('CLI source inventory differs; review before refreshing with --write');
console.log(`Recorded ${rows.length} pinned hand-written command registrations and ${Object.keys(aliases).length} compatibility group aliases.`);
