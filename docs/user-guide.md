# Cortex User Guide

Cortex is an AI-powered coding assistant that helps you write, review, debug, and understand code. It operates through a command-line interface with both interactive and automated modes.

## Introduction

Cortex provides:

- **Intelligent Code Assistance** - Get help writing, reviewing, and debugging code
- **Interactive Conversations** - Continuous dialogue for complex tasks
- **Automated Execution** - Headless mode for CI/CD and scripting
- **Plugin Extensibility** - Extend functionality with WASM plugins
- **Multi-Model Support** - Use various AI models based on your needs

## Installation

### Linux and macOS

> **Security Note:** Before running any installation script, you can review it first:
> ```bash
> curl -fsSL https://software.cortex.foundation/install.sh | less
> ```

```bash
curl -fsSL https://software.cortex.foundation/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://software.cortex.foundation/install.ps1 | iex
```

### Verify Installation

```bash
cortex --version
```

## Quick Start

### Start Interactive Session

Launch Cortex in interactive mode:

```bash
cortex
```

### Run a Single Prompt

Execute a one-off prompt:

```bash
cortex "explain this codebase"
```

### Exec Mode (Non-Interactive)

Run automated tasks:

```bash
cortex exec "fix the bug in main.rs"
```

## Interactive vs Exec Mode

| Feature | Interactive Mode | Exec Mode |
|---------|-----------------|-----------|
| User Interaction | ✅ Continuous dialogue | ❌ One-shot execution |
| Approval Prompts | ✅ Interactive approval | ⚙️ Configurable via `--auto` |
| Session Persistence | ✅ Automatic | ⚙️ Optional via `--session-id` |
| Best For | Development, exploration | CI/CD, automation |
| Output Format | Rich TUI | Text, JSON, JSONL, Markdown |
| Timeout | None | Configurable (default: 600s) |

### When to Use Interactive Mode

- Exploring a new codebase
- Iterative development and debugging
- Learning and experimentation
- Complex tasks requiring back-and-forth

### When to Use Exec Mode

- CI/CD pipelines
- Automated testing and review
- Shell scripts and batch processing
- Scheduled tasks

## Interactive Mode

### Starting a Session

```bash
# Start in current directory
cortex

# Start in specific directory
cortex --cwd /path/to/project

# Start with a specific model
cortex -m claude-3-opus
```

### Common Commands

Within an interactive session, you can use slash commands:

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/clear` | Clear the conversation |
| `/model <name>` | Switch AI model |
| `/export` | Export the session |
| `/quit` or `/exit` | Exit Cortex |

### Navigation

- **Arrow keys** - Navigate history
- **Tab** - Auto-complete
- **Ctrl+C** - Cancel current operation
- **Ctrl+D** - Exit session

## Working with Sessions

Sessions preserve conversation history and context across interactions.

### Automatic Sessions

In interactive mode, sessions are automatically created and saved.

### Continue a Session

Resume a previous session:

```bash
cortex --session-id <session-id>
```

### Export a Session

Save session to a file:

```bash
cortex export --session-id <session-id> -o session.json
```

### Import a Session

Load a saved session:

```bash
cortex import session.json
```

### List Sessions

View available sessions:

```bash
cortex sessions list
```

## Model Selection

Cortex supports multiple AI models for different use cases.

### List Available Models

```bash
cortex models list
```

### Use a Specific Model

```bash
# Interactive mode
cortex -m claude-3-opus

# Exec mode
cortex exec -m gpt-4-turbo "your prompt"

# During session (slash command)
/model claude-3-sonnet
```

### Model Recommendations

| Use Case | Recommended Model |
|----------|------------------|
| Complex reasoning | claude-3-opus, gpt-4 |
| Fast responses | claude-3-haiku, gpt-3.5-turbo |
| Code generation | claude-3-opus, gpt-4-turbo |
| Simple tasks | Any fast model |

## Common Workflows

### Code Review

```bash
# Interactive
cortex
> Review this code for bugs and security issues

# Exec mode
cortex exec --auto read-only "Review this PR for security vulnerabilities"
```

### Documentation Generation

```bash
# Generate docs for a file
cortex exec --auto low "Generate documentation for src/lib.rs"

# Generate README
cortex "Create a README for this project"
```

### Debugging

```bash
# Interactive debugging session
cortex
> Help me debug this error: [paste error message]

# With context
cortex --include "src/main.rs" "Why is this function returning null?"
```

### Refactoring

```bash
# Plan refactoring
cortex "How should I refactor the auth module?"

# Execute refactoring
cortex exec --auto medium "Refactor the auth module to use dependency injection"
```

### Testing

```bash
# Generate tests
cortex exec --auto low "Write unit tests for src/utils.rs"

# Analyze test coverage
cortex "Analyze test coverage and suggest improvements"
```

### Code Explanation

```bash
# Explain a file
cortex "Explain what src/main.rs does"

# Explain a function
cortex "Explain the authenticate() function in auth.rs"

# Explain architecture
cortex "Explain the overall architecture of this project"
```

## Including Context

### Include Files

Add files to the conversation context:

```bash
# Specific files
cortex --include "src/main.rs" "Explain this code"

# Multiple files
cortex --include "src/*.rs" --exclude "*_test.rs" "Review these files"
```

### Include Git Diff

Include uncommitted changes:

```bash
cortex --git-diff "Review my changes"
```

### Include URLs

Fetch and include web content:

```bash
cortex --url "https://docs.example.com/api" "How do I use this API?"
```

### Include Images

Attach images for analysis:

```bash
cortex -i screenshot.png "What's wrong with this UI?"
```

## Configuration

### Configuration File Location

```
~/.cortex/config.toml
```

### Common Configuration Options

```toml
# Default model
default_model = "claude-3-opus"

# Theme settings
theme = "dark"

# Session settings
auto_save_sessions = true

# Plugin settings
plugins_enabled = true
```

### Environment Variables

Override settings with environment variables:

```bash
export CORTEX_MODEL="claude-3-opus"
export CORTEX_API_KEY="your-api-key"
```

### CLI Flag Precedence

Configuration precedence (highest to lowest):
1. CLI flags
2. Environment variables
3. Config file
4. Defaults

## Updating Cortex

### Upgrade to Latest Version

```bash
cortex upgrade
```

### Check Current Version

```bash
cortex --version
```

## Plugins

Cortex supports plugins to extend functionality.

### List Installed Plugins

```bash
cortex plugin list
```

### Install a Plugin

```bash
cortex plugin install <plugin-name>
```

### Enable/Disable Plugins

```bash
cortex plugin enable <plugin-name>
cortex plugin disable <plugin-name>
```

For detailed plugin development information, see the [Plugin Development Guide](plugins.md).

## Exec Mode Details

For comprehensive exec mode documentation, see the [Exec Mode Guide](exec-mode.md).

### Quick Reference

```bash
# Basic execution
cortex exec "your prompt"

# From file
cortex exec -f prompt.txt

# With autonomy level
cortex exec --auto medium "implement feature"

# With JSON output
cortex exec -o json "analyze code" | jq '.response'

# With timeout
cortex exec --timeout 300 "quick task"
```

## Tips and Best Practices

### 1. Be Specific in Prompts

```bash
# Good
cortex "Add error handling to the parse_config function in src/config.rs"

# Less Good
cortex "Fix the config file"
```

### 2. Provide Context

```bash
# Include relevant files
cortex --include "src/auth/*.rs" "How does authentication work?"

# Mention constraints
cortex "Refactor this code, keeping backward compatibility"
```

### 3. Use Appropriate Autonomy

```bash
# Start safe
cortex exec --auto read-only "analyze code"

# Increase as needed
cortex exec --auto medium "implement changes"
```

### 4. Review Before Committing

```bash
# Check changes
git diff

# Review with Cortex
cortex --git-diff "Review these changes before I commit"
```

### 5. Save Complex Sessions

```bash
# Export important sessions
cortex export --session-id <id> -o project-analysis.json
```

## Getting Help

### In-App Help

```bash
# General help
cortex --help

# Command-specific help
cortex exec --help
cortex plugin --help
```

### Slash Commands

In interactive mode:

```
/help
```

## Troubleshooting

### API Key Issues

Ensure your API key is configured:

```bash
# Check environment variable
echo $CORTEX_API_KEY

# Or configure in config file
# ~/.cortex/config.toml
```

### Connection Issues

Check network connectivity:

```bash
cortex exec -v "test connection"
```

### Session Issues

Clear session cache if experiencing problems:

```bash
# Start fresh session
cortex --new-session
```

### Plugin Issues

Disable problematic plugins:

```bash
cortex plugin disable <plugin-name>
```

### Performance Issues

Use a faster model for simple tasks:

```bash
cortex -m claude-3-haiku "quick question"
```

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+C` | Cancel current operation |
| `Ctrl+D` | Exit session |
| `Ctrl+L` | Clear screen |
| `Up/Down` | Navigate history |
| `Tab` | Auto-complete |

## Support

For additional help:

- **Documentation**: Check the docs folder
- **Issues**: Report bugs on the project repository
- **Community**: Join community discussions

---

Happy coding with Cortex!
