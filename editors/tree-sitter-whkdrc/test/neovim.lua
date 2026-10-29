-- Checks the queries in Neovim's own tree-sitter engine. Run from this directory with
--
--   nvim --headless -l test/neovim.lua <parser.dll> <fixture> [<fixture> ...]
--
-- (build the parser first with `tree-sitter build -o <parser.dll>`). For each fixture it prints the
-- language each command is injected as, and which highlight captures fire, and exits non-zero if a
-- query does not compile or the fixture has a syntax error.

local dll = arg[1]
local fixtures = { unpack(arg, 2) }
assert(dll and #fixtures > 0, 'usage: nvim --headless -l test/neovim.lua <parser.dll> <fixture>...')

vim.treesitter.language.add('whkdrc', { path = dll })

local function read(path)
  return table.concat(vim.fn.readfile(path), '\n')
end

local here = vim.fs.dirname(vim.fs.normalize(debug.getinfo(1, 'S').source:sub(2)))
local queries = here .. '/../queries'

local failed = false

for _, fixture in ipairs(fixtures) do
  local text = read(fixture)
  local parser = vim.treesitter.get_string_parser(text, 'whkdrc')
  local root = parser:parse()[1]:root()

  print('== ' .. vim.fs.basename(fixture))

  if root:has_error() then
    print('  syntax error')
    failed = true
  end

  -- every command is embedded as the language of the .shell directive
  local injections = vim.treesitter.query.parse('whkdrc', read(queries .. '/injections.scm'))
  local languages = {}
  for _, match, metadata in injections:iter_matches(root, text, 0, -1, { all = true }) do
    for id, nodes in pairs(match) do
      if injections.captures[id] == 'injection.content' then
        for _, node in ipairs(nodes) do
          local language = metadata['injection.language']
          languages[language] = (languages[language] or 0) + 1
          local line = vim.treesitter.get_node_text(node, text):gsub('\n', ' ')
          print(('  inject %-10s %s'):format(language, line))
        end
      end
    end
  end

  -- the highlight query compiles, and says what it finds
  local highlights = vim.treesitter.query.parse('whkdrc', read(queries .. '/highlights.scm'))
  local counts = {}
  for id in highlights:iter_captures(root, text, 0, -1) do
    local name = highlights.captures[id]
    counts[name] = (counts[name] or 0) + 1
  end
  local names = vim.tbl_keys(counts)
  table.sort(names)
  print('  captures: ' .. table.concat(vim.tbl_map(function(n) return n .. '=' .. counts[n] end, names), ' '))
end

vim.treesitter.query.parse('whkdrc', read(queries .. '/folds.scm'))

if failed then
  os.exit(1)
end
