# Lintrunner LSP Server

This directory contains the Language Server Protocol (LSP) implementation for lintrunner, providing real-time linting feedback in editors that support LSP.

## Features

- **Real-time diagnostics**: Get lint errors and warnings as you type
- **Automatic file watching**: Diagnostics update when files change
- **Code actions**: Quick fixes for lint issues (when available)
- **Multi-linter support**: Works with all linters configured in `.lintrunner.toml`

## Building

To build the LSP server, enable the `lsp` feature:

```bash
cargo build --features lsp --bin lintrunner-lsp
```

## Usage

The LSP server communicates over stdin/stdout using the Language Server Protocol. Most editors will handle this automatically when configured.

### VS Code

Add this to your VS Code settings or create a custom extension:

```json
{
  "lintrunner-lsp": {
    "command": "lintrunner-lsp",
    "filetypes": ["*"]
  }
}
```

### Neovim

Using nvim-lspconfig:

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

-- Define the lintrunner LSP server
if not configs.lintrunner then
  configs.lintrunner = {
    default_config = {
      cmd = {'lintrunner-lsp'},
      filetypes = {'*'},
      root_dir = lspconfig.util.root_pattern('.lintrunner.toml'),
      settings = {},
    },
  }
end

-- Setup the server
lspconfig.lintrunner.setup{}
```

### Emacs (eglot)

```elisp
(add-to-list 'eglot-server-programs
             '((python-mode rust-mode js-mode) . ("lintrunner-lsp")))
```

## Configuration

The LSP server automatically loads configuration from `.lintrunner.toml` in the workspace root. No additional configuration is required.

## Architecture

The LSP server:

1. Loads the lintrunner configuration on initialization
2. Runs applicable linters when files are opened, changed, or saved
3. Converts `LintMessage` structs to LSP `Diagnostic` objects
4. Publishes diagnostics to the editor
5. Provides code actions for fixes when available

## Limitations

- Code actions for applying fixes are not yet fully implemented
- File watching for configuration changes is not implemented
- Performance may be impacted on large codebases with many linters