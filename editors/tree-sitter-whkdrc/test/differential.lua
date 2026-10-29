-- Used by the differential test in xtask (`cargo test -p xtask -- --ignored`): reads a JSON array of
-- sources and writes a JSON array saying whether each one has a syntax error.
--
--   nvim --headless -l test/differential.lua <parser.dll> <sources.json> <results.json>

local dll, input, output = arg[1], arg[2], arg[3]
assert(dll and input and output, 'usage: nvim --headless -l test/differential.lua <parser.dll> <in> <out>')

vim.treesitter.language.add('whkdrc', { path = dll })

local sources = vim.json.decode(table.concat(vim.fn.readfile(input), '\n'))
local results = {}

for i, text in ipairs(sources) do
  local parser = vim.treesitter.get_string_parser(text, 'whkdrc')
  results[i] = parser:parse()[1]:root():has_error()
end

vim.fn.writefile({ vim.json.encode(results) }, output)
