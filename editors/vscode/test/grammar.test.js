// Tokenises the fixtures with the TextMate grammar, using the same engine as VS Code, and checks
// the scopes. Run with `npm install && npm test` in editors/vscode.
//
// The grammar is generated (`cargo xtask editors`); these tests are what keep its hand-written
// structure honest, together with the differential test against the real parser.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vsctm = require('vscode-textmate');
const oniguruma = require('vscode-oniguruma');

const grammarPath = path.join(__dirname, '..', 'syntaxes', 'whkdrc.tmLanguage.json');
const fixtures = path.join(__dirname, '..', '..', 'fixtures');

// VS Code ships grammars for PowerShell and batch. They are only needed to check that the embedding
// really switches language, so those checks are skipped if VS Code cannot be found.
function builtinGrammar(extension, file) {
  const dirs = [process.env.WHKD_VSCODE_EXTENSIONS];
  for (const entry of (process.env.PATH || '').split(path.delimiter)) {
    for (const name of ['code', 'code.cmd']) {
      if (entry && fs.existsSync(path.join(entry, name))) {
        dirs.push(path.join(entry, '..', 'resources', 'app', 'extensions'));
      }
    }
  }
  for (const dir of dirs.filter(Boolean)) {
    const candidate = path.join(dir, extension, 'syntaxes', file);
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}

const embedded = {
  'source.powershell': builtinGrammar('powershell', 'powershell.tmLanguage.json'),
  'source.batchfile': builtinGrammar('bat', 'batchfile.tmLanguage.json'),
};

const wasm = fs.readFileSync(require.resolve('vscode-oniguruma/release/onig.wasm')).buffer;
const onigLib = oniguruma.loadWASM(wasm).then(() => ({
  createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
  createOnigString: (s) => new oniguruma.OnigString(s),
}));

const registry = new vsctm.Registry({
  onigLib,
  loadGrammar: async (scope) => {
    const file = scope === 'source.whkdrc' ? grammarPath : embedded[scope];
    return file ? vsctm.parseRawGrammar(fs.readFileSync(file, 'utf8'), file) : null;
  },
});

let grammarPromise;
function grammar() {
  grammarPromise ??= registry.loadGrammar('source.whkdrc');
  return grammarPromise;
}

// the tokens of every line, with the text and scopes of each
async function tokenize(source) {
  const g = await grammar();
  let stack = vsctm.INITIAL;
  return source.split(/\r?\n/).map((line) => {
    const result = g.tokenizeLine(line, stack);
    stack = result.ruleStack;
    return result.tokens.map((t) => ({
      text: line.slice(t.startIndex, Math.min(t.endIndex, line.length)),
      scopes: t.scopes,
    }));
  });
}

async function fixture(name) {
  return tokenize(fs.readFileSync(path.join(fixtures, name), 'utf8'));
}

// the first token on a line whose text is exactly `text`
function token(line, text, nth = 0) {
  const found = line.filter((t) => t.text === text);
  assert.ok(found.length > nth, `no token ${nth} with text ${JSON.stringify(text)} in ${JSON.stringify(line.map((t) => t.text))}`);
  return found[nth];
}

function has(tok, scope) {
  return tok.scopes.some((s) => s === scope || s.startsWith(`${scope}.`));
}

function assertScope(line, text, scope, nth = 0) {
  const tok = token(line, text, nth);
  assert.ok(has(tok, scope), `${JSON.stringify(text)} should be ${scope}, but is ${tok.scopes.join(' ')}`);
}

function assertNotScope(line, text, scope, nth = 0) {
  const tok = token(line, text, nth);
  assert.ok(!has(tok, scope), `${JSON.stringify(text)} should not be ${scope}, but is ${tok.scopes.join(' ')}`);
}

const rest = (line) => line.map((t) => t.text).join('');

// the first line whose text contains `text` (tokens may be split finer than `text`)
function lineWith(lines, text) {
  const found = lines.find((line) => rest(line).includes(text));
  assert.ok(found, `no line containing ${JSON.stringify(text)}`);
  return found;
}

// the first token containing `needle` has the scope (the embedded shell grammar may split a command
// into several tokens, so a command is checked by one of its words)
function assertContains(line, needle, scope) {
  const tok = line.find((t) => t.text.includes(needle));
  assert.ok(tok, `no token containing ${JSON.stringify(needle)} in ${JSON.stringify(rest(line))}`);
  assert.ok(has(tok, scope), `${JSON.stringify(needle)} should be ${scope}, but is ${tok.scopes.join(' ')}`);
}

function assertNotContains(line, needle, scope) {
  const tok = line.find((t) => t.text.includes(needle));
  assert.ok(tok, `no token containing ${JSON.stringify(needle)} in ${JSON.stringify(rest(line))}`);
  assert.ok(!has(tok, scope), `${JSON.stringify(needle)} should not be ${scope}, but is ${tok.scopes.join(' ')}`);
}

test('the grammar is valid json with a scope name', () => {
  const raw = JSON.parse(fs.readFileSync(grammarPath, 'utf8'));
  assert.equal(raw.scopeName, 'source.whkdrc');
});

test('directives, shell, and trailing comments', async () => {
  const lines = await fixture('basic.whkdrc');

  assertScope(lines[0], '.shell', 'keyword.control.directive');
  assertScope(lines[0], 'pwsh', 'constant.language.shell');
  assertScope(lines[0], '# can be one of cmd | pwsh | powershell', 'comment.line');

  assertScope(lines[1], '.pause', 'keyword.control.directive');
  assertScope(lines[1], 'alt', 'keyword.other.modifier');
  assertScope(lines[1], 'shift', 'keyword.other.modifier');
  assertScope(lines[1], 'p', 'constant.character.key');
  assertScope(lines[1], '+', 'keyword.operator.key-separator');

  assertScope(lines[2], '.pause_hook', 'keyword.control.directive');
  assertNotScope(lines[2], '.pause_hook', 'constant.character.key');
});

test('bindings: keys, modifiers, the delimiter and the command', async () => {
  const lines = await fixture('basic.whkdrc');
  const binding = lineWith(lines, 'komorebic focus left');

  assertScope(binding, 'alt', 'keyword.other.modifier');
  assertScope(binding, 'h', 'constant.character.key');
  assertScope(binding, ':', 'punctuation.separator.delimiter');
  assertContains(binding, 'komorebic', 'meta.embedded.line.powershell');
  assertScope(binding, '# a trailing comment', 'comment.line');
  assertNotScope(binding, '# a trailing comment', 'meta.embedded.line.powershell');

  assertScope(lineWith(lines, 'focus-workspace'), '1', 'constant.character.key');
  assertScope(lineWith(lines, 'toggle-maximize'), 'f11', 'constant.character.key');
});

test('blocks: process names, predicates, operators and keywords', async () => {
  const lines = await fixture('blocks.whkdrc');

  const open = lineWith(lines, 'alt + n');
  assertScope(open, '[', 'punctuation.definition.block.begin');
  assertScope(lineWith(lines, 'ProcessName as shown'), '# ProcessName as shown by `Get-Process`', 'comment.line');

  assertScope(lineWith(lines, 'hello firefox'), 'Firefox', 'entity.name.type.process');
  assertScope(lineWith(lines, 'hello chrome'), 'Google Chrome', 'entity.name.type.process');

  const apps = lineWith(lines, 'zen or code');
  assertScope(apps, 'app', 'variable.parameter.condition');
  assertScope(apps, '=', 'keyword.operator.comparison');
  assertScope(apps, 'Zen Browser', 'entity.name.type.process');
  assertScope(apps, '|', 'keyword.operator.logical');
  assertScope(apps, 'Code', 'entity.name.type.process');

  const layouts = lineWith(lines, 'komorebic focus left');
  assertScope(layouts, 'layout', 'variable.parameter.condition');
  assertScope(layouts, 'bsp', 'constant.other.layout');
  assertScope(layouts, 'grid', 'constant.other.layout');

  const both = lineWith(lines, 'komorebic focus up');
  assertScope(both, '&', 'keyword.operator.logical');
  assertScope(both, '!=', 'keyword.operator.comparison');
  assertScope(both, 'rows', 'constant.other.layout');

  // layout names are accepted in the spelling komorebi itself uses too
  assertScope(lineWith(lines, 'echo ultrawide'), 'UltrawideVerticalStack', 'constant.other.layout');

  assertScope(lineWith(lines, 'komorebic close'), 'Default', 'keyword.other.default');
  assertScope(lineWith(lines, 'Ignore'), 'Ignore', 'keyword.other.ignore');
  assertScope(lineWith(lines, '# a comment after the bracket'), '# a comment after the bracket', 'comment.line');

  // the block ends at `]`, after which bindings are plain again
  const plain = lineWith(lines, 'a plain binding');
  assertScope(plain, 'alt', 'keyword.other.modifier');
  assertScope(plain, 'f', 'constant.character.key');
  assertNotScope(plain, 'f', 'entity.name.type.process');
});

test('when guards use the same conditions as blocks', async () => {
  const lines = await fixture('when.whkdrc');

  const scrolling = lineWith(lines, 'consume left');
  assertScope(scrolling, 'when', 'keyword.control.when');
  assertScope(scrolling, 'layout', 'variable.parameter.condition');
  assertScope(scrolling, 'scrolling', 'constant.other.layout');
  assertScope(scrolling, ':', 'punctuation.separator.delimiter');
  assertContains(scrolling, 'komorebic', 'meta.embedded.line.powershell');

  const guarded = lineWith(lines, 'echo guarded');
  assertScope(guarded, 'app', 'variable.parameter.condition');
  assertScope(guarded, 'Firefox', 'entity.name.type.process');
  assertScope(guarded, '&', 'keyword.operator.logical');
  assertScope(guarded, '!=', 'keyword.operator.comparison');
  assertScope(guarded, 'bsp', 'constant.other.layout');

  assertScope(lineWith(lines, 'a bare process name'), 'Firefox', 'entity.name.type.process');
});

test('commands follow the .shell directive', async () => {
  const cmd = await fixture('cmd.whkdrc');
  assertScope(cmd[0], 'cmd', 'constant.language.shell');
  assertScope(cmd[0], '.shell', 'meta.shell.batch');
  assertContains(lineWith(cmd, 'echo hello'), 'echo', 'meta.embedded.line.batchfile');
  assertContains(lineWith(cmd, 'echo done'), 'echo', 'meta.embedded.line.batchfile');

  const ps = await fixture('basic.whkdrc');
  assertScope(ps[0], '.shell', 'meta.shell.powershell');
  assertContains(lineWith(ps, 'focus down'), 'komorebic', 'meta.embedded.line.powershell');
  assertNotContains(lineWith(ps, 'focus down'), 'komorebic', 'meta.embedded.line.batchfile');
});

test('commands are handed to the shell grammars when they are available', async (t) => {
  if (!embedded['source.powershell'] || !embedded['source.batchfile']) {
    t.skip('VS Code (and its PowerShell and batch grammars) was not found');
    return;
  }

  // an included grammar contributes its own scopes (such as `support.function.powershell`), not
  // its root scope name, so look for any scope from it beyond the embedding scope itself
  const fromShell = (line, suffix) =>
    line.flatMap((t) => t.scopes).filter((s) => s.endsWith(suffix) && !s.startsWith('meta.embedded'));

  const ps = await fixture('basic.whkdrc');
  const powershell = fromShell(lineWith(ps, 'Write-Host'), '.powershell');
  assert.ok(powershell.length > 0, 'PowerShell should have tokenised the command');

  const cmd = await fixture('cmd.whkdrc');
  const batch = fromShell(lineWith(cmd, 'echo hello'), '.batchfile');
  assert.ok(batch.length > 0, 'batch should have tokenised the command');
  assert.equal(fromShell(lineWith(cmd, 'echo hello'), '.powershell').length, 0);
});

test('tricky cases: a key named when, # ends a command, empty commands', async () => {
  const lines = await fixture('tricky.whkdrc');

  const key = lineWith(lines, 'a key named when');
  assertScope(key, 'when', 'constant.character.key');
  assertNotScope(key, 'when', 'keyword.control.when');

  const guard = lineWith(lines, 'a guard on a key named when');
  assertScope(guard, 'when', 'constant.character.key', 0);
  assertScope(guard, 'when', 'keyword.control.when', 1);

  const hash = lineWith(lines, 'before');
  assertScope(hash, '# this is a comment, not part of the command', 'comment.line');
  assertNotScope(hash, '# this is a comment, not part of the command', 'meta.embedded.line.powershell');

  const empty = lines.find((line) => rest(line).startsWith('alt + e :'));
  assertScope(empty, ':', 'punctuation.separator.delimiter');

  assertScope(lineWith(lines, 'digits are keys'), '5', 'constant.character.key');
  assertScope(lineWith(lines, 'oem'), 'oem_plus', 'constant.character.key');
  assertScope(lineWith(lines, 'oem'), 'ctrl', 'keyword.other.modifier');
  assertScope(lineWith(lines, 'a process name with a space'), 'Google Chrome', 'entity.name.type.process');
});

test('mouse inputs are highlighted as keys of their own', async () => {
  const lines = await fixture('mouse.whkdrc');

  const wheel = lineWith(lines, 'komorebic focus left');
  assertScope(wheel, 'alt', 'keyword.other.modifier');
  assertScope(wheel, 'wheelup', 'constant.character.key.mouse');
  assertScope(lineWith(lines, 'komorebic focus right'), 'wheeldown', 'constant.character.key.mouse');

  // the cooldown directive, with its number
  const cooldown = lineWith(lines, '.wheel_cooldown');
  assertScope(cooldown, '.wheel_cooldown', 'keyword.control.directive');
  assertScope(cooldown, '150', 'constant.numeric');
  assertScope(cooldown, '# milliseconds', 'comment.line');
  const gestures = lineWith(lines, '.gesture_cooldown');
  assertScope(gestures, '.gesture_cooldown', 'keyword.control.directive');
  assertScope(gestures, '300', 'constant.numeric');

  // in any case, and with several modifiers
  const middle = lineWith(lines, 'toggle-monocle');
  assertScope(middle, 'Alt', 'keyword.other.modifier');
  assertScope(middle, 'Shift', 'keyword.other.modifier');
  assertScope(middle, 'MButton', 'constant.character.key.mouse');

  assertScope(lineWith(lines, 'cycle-workspace'), 'xbutton1', 'constant.character.key.mouse');

  // with a guard, and as the trigger of a block
  const guarded = lineWith(lines, 'scroll-left');
  assertScope(guarded, 'wheelup', 'constant.character.key.mouse');
  assertScope(guarded, 'when', 'keyword.control.when');
  assertScope(lineWith(lines, 'alt + shift + wheeldown ['), 'wheeldown', 'constant.character.key.mouse');

  // a keyboard key is not a mouse input
  const keyboard = lineWith(await fixture('basic.whkdrc'), 'focus left');
  assertNotScope(keyboard, 'h', 'constant.character.key.mouse');
});

test('gestures are highlighted as keys of their own', async () => {
  const lines = await fixture('gestures.whkdrc');

  const stream = lineWith(lines, 'komorebic pan --stream');
  assertScope(stream, 'swipe3', 'constant.character.key.gesture');

  const guarded = lineWith(lines, 'alt + swipe4 when');
  assertScope(guarded, 'alt', 'keyword.other.modifier');
  assertScope(guarded, 'swipe4', 'constant.character.key.gesture');
  assertScope(guarded, 'when', 'keyword.control.when');

  assertScope(lineWith(lines, 'komorebic focus left'), 'swipe3left', 'constant.character.key.gesture');
  assertScope(lineWith(lines, 'komorebic focus right'), 'Swipe3Right', 'constant.character.key.gesture');
  assertScope(lineWith(lines, 'toggle-monocle'), 'swipe4up', 'constant.character.key.gesture');
  assertScope(lineWith(lines, 'swipe3down ['), 'swipe3down', 'constant.character.key.gesture');

  // a gesture is not a mouse input, and a mouse input is not a gesture
  assertNotScope(stream, 'swipe3', 'constant.character.key.mouse');
  const wheel = lineWith(await fixture('mouse.whkdrc'), 'komorebic focus left');
  assertNotScope(wheel, 'wheelup', 'constant.character.key.gesture');
});

test('mistakes are marked invalid, like the parser rejects them', async () => {
  const layouts = await fixture('invalid/unknown-layout.whkdrc');
  assertScope(lineWith(layouts, 'a typo'), 'bpsp', 'invalid.illegal.unknown-layout');

  const mixed = lineWith(layouts, 'one of three');
  assertScope(mixed, 'bsp', 'constant.other.layout');
  assertScope(mixed, 'grid', 'constant.other.layout');
  assertScope(mixed, 'nope', 'invalid.illegal.unknown-layout');

  assertScope(lineWith(layouts, 'condition'), 'layot', 'invalid.illegal.unknown-condition');

  const shell = await fixture('invalid/unknown-shell.whkdrc');
  assertScope(shell[0], 'zsh', 'invalid.illegal.unknown-shell');
});

test('without a valid .shell the commands are plain strings', async () => {
  const lines = await fixture('invalid/no-shell.whkdrc');
  const binding = lines[0];
  assertScope(binding, 'alt', 'keyword.other.modifier');
  const command = binding.find((t) => t.text.includes('no shell directive'));
  assert.ok(has(command, 'string.unquoted.command'), command.scopes.join(' '));
  assert.ok(!command.scopes.some((s) => s.startsWith('meta.embedded')));
});

test('valid fixtures contain nothing marked invalid', async () => {
  for (const name of ['basic', 'blocks', 'when', 'cmd', 'tricky', 'mouse', 'gestures']) {
    const lines = await fixture(`${name}.whkdrc`);
    for (const line of lines) {
      for (const tok of line) {
        assert.ok(
          !tok.scopes.some((s) => s.startsWith('invalid')),
          `${name}: ${JSON.stringify(tok.text)} is ${tok.scopes.join(' ')}`,
        );
      }
    }
  }
});

test('CRLF files tokenise the same', async () => {
  const source = fs.readFileSync(path.join(fixtures, 'blocks.whkdrc'), 'utf8').replace(/\n/g, '\r\n');
  const lines = await tokenize(source);
  assertScope(lineWith(lines, 'hello firefox'), 'Firefox', 'entity.name.type.process');
  assertScope(lineWith(lines, 'komorebic focus left'), 'bsp', 'constant.other.layout');
});
