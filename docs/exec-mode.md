# Exec Mode Guide

Exec mode enables non-interactive, headless execution of Cortex for automation, CI/CD pipelines, and scripting workflows.

## Overview

Exec mode is designed for scenarios where human interaction is not available or desired:

- **CI/CD Pipelines** - Automated code review, testing, and deployment
- **Shell Scripts** - Batch processing and automation
- **Scheduled Tasks** - Cron jobs and periodic maintenance
- **Programmatic Integration** - API-like usage from other tools

Key characteristics:
- One-shot execution: complete the task and exit
- Configurable autonomy levels for safety
- Structured output formats (text, JSON, JSONL, Markdown)
- Timeout and turn limits to prevent runaway execution

## Basic Usage

### Direct Prompt

```bash
cortex exec "your prompt here"
```

### From File

```bash
cortex exec -f prompt.txt
cortex exec --file prompt.txt
```

### From Standard Input

```bash
echo "your prompt" | cortex exec
cat instructions.md | cortex exec
```

### Combining Methods

```bash
# Prompt with context from stdin
echo "additional context" | cortex exec "analyze this code"
```

## CLI Options Reference

### Input Options

| Flag | Description |
|------|-------------|
| `<prompt>` | The prompt to execute (positional argument) |
| `-f, --file <PATH>` | Read prompt from a file |
| `--input-format <FORMAT>` | Input format for multi-turn: `text`, `json` |
| `--clipboard` | Read input from clipboard |
| `--git-diff` | Include git diff in context |
| `--include <PATTERN>` | Include files matching pattern in context |
| `--exclude <PATTERN>` | Exclude files matching pattern |
| `--url <URL>` | Fetch and include URL content in context |
| `-i, --image <PATH>` | Attach image file (can be used multiple times) |

### Output Options

| Flag | Description |
|------|-------------|
| `-o, --output-format <FORMAT>` | Output format: `text` (default), `json`, `jsonl`, `markdown` |
| `--echo` | Include the prompt in the output |
| `--response-format <FORMAT>` | Response format: `text`, `json`, `json_object` |
| `--output-schema <SCHEMA>` | JSON schema for structured output |
| `--logprobs <N>` | Request log probabilities (number of tokens) |

### Model Options

| Flag | Description |
|------|-------------|
| `-m, --model <MODEL>` | Model ID to use (e.g., `claude-3-opus`) |
| `--spec-model <MODEL>` | Model for specification mode |
| `--use-spec` | Start in specification mode |
| `-r, --reasoning-effort <LEVEL>` | Reasoning effort level |
| `--max-tokens <N>` | Maximum tokens in response |

### Execution Control

| Flag | Description |
|------|-------------|
| `--auto <LEVEL>` | Autonomy level: `read-only` (default), `low`, `medium`, `high` |
| `--skip-permissions-unsafe` | Skip ALL permission checks (DANGEROUS) |
| `--max-turns <N>` | Maximum turns (default: 100) |
| `--timeout <SECS>` | Timeout in seconds (default: 600, 0 for unlimited) |
| `--cwd <PATH>` | Working directory for execution |

### Tool Options

| Flag | Description |
|------|-------------|
| `--enabled-tools <TOOLS>` | Enable specific tools (comma-separated) |
| `--disabled-tools <TOOLS>` | Disable specific tools (comma-separated) |
| `--list-tools` | List available tools and exit |

### Session Options

| Flag | Description |
|------|-------------|
| `-s, --session-id <ID>` | Continue an existing session |
| `--user <ID>` | User identifier for tracking |

### Advanced Options

| Flag | Description |
|------|-------------|
| `-v, --verbose` | Enable verbose output |
| `--system <PROMPT>` | Custom system prompt |
| `--suffix <TEXT>` | Suffix for completion mode |
| `--frequency-penalty <N>` | Frequency penalty (-2.0 to 2.0) |
| `--presence-penalty <N>` | Presence penalty (-2.0 to 2.0) |
| `--stop <SEQ>` | Stop sequence (can be used multiple times) |
| `-n, --n <N>` | Number of completions to generate |
| `--best-of <N>` | Generate best_of completions and return the best |

## Autonomy Levels

The `--auto` flag controls what actions Cortex can perform without explicit approval.

| Level | Description | Allowed Operations |
|-------|-------------|-------------------|
| `read-only` | No modifications (default) | Read files, search, analyze |
| `low` | Basic file operations | Read, write files, formatting |
| `medium` | Development operations | Package install, builds, local git |
| `high` | Full access | Everything including git push, deployments |

### Choosing an Autonomy Level

**`read-only`** (safest)
- Code review and analysis
- Documentation generation (read-only)
- Architecture exploration
- Planning and estimation

```bash
cortex exec --auto read-only "Analyze this codebase for security vulnerabilities"
```

**`low`**
- Documentation updates
- Code formatting
- Simple file modifications
- Configuration updates

```bash
cortex exec --auto low "Fix all formatting issues in src/"
```

**`medium`**
- Feature implementation
- Bug fixes
- Test writing
- Local git operations (commit, branch)

```bash
cortex exec --auto medium "Implement unit tests for the auth module"
```

**`high`**
- CI/CD deployments
- Remote git operations (push, merge)
- Database migrations
- Production operations

```bash
cortex exec --auto high "Deploy to staging and run integration tests"
```

### Skip Permissions (DANGEROUS)

> ⚠️ **EXTREME CAUTION REQUIRED** ⚠️
>
> The `--skip-permissions-unsafe` flag is **inherently dangerous** and should be avoided in almost all cases. Using this flag can lead to:
> - Unintended file deletions or modifications
> - Exposure of sensitive data
> - System-wide changes that are difficult to reverse
> - Security vulnerabilities in your environment

The `--skip-permissions-unsafe` flag bypasses **ALL** permission checks:

```bash
# ⚠️ DANGEROUS: Use only in fully isolated, ephemeral environments
# Never use this on production systems or with sensitive data
cortex exec --skip-permissions-unsafe "full system access task"
```

**When is this acceptable?**
- Isolated Docker containers that are discarded after use
- Ephemeral CI/CD runners with no sensitive data
- Sandboxed testing environments

**When should you NEVER use this?**
- Production systems
- Any environment with sensitive data or credentials
- Shared development machines
- When processing untrusted input

## Output Formats

### Text (Default)

Plain text output suitable for human reading:

```bash
cortex exec "explain this function"
```

### JSON

Structured JSON output for programmatic consumption:

```bash
cortex exec -o json "list all TODO comments" | jq '.response'
```

Output structure:
```json
{
  "response": "The AI's response text",
  "tool_calls": [...],
  "usage": {
    "prompt_tokens": 100,
    "completion_tokens": 50
  }
}
```

### JSONL

Newline-delimited JSON for streaming and log processing:

```bash
cortex exec -o jsonl "analyze code"
```

### Markdown

Markdown-formatted output:

```bash
cortex exec -o markdown "document this API" > api-docs.md
```

## Timeouts and Limits

### Timeout

Default: 600 seconds (10 minutes)

```bash
# Set custom timeout
cortex exec --timeout 1800 "long running task"

# No timeout (be careful!)
cortex exec --timeout 0 "indefinite task"
```

### Max Turns

Default: 100 turns

```bash
# Limit to 10 turns
cortex exec --max-turns 10 "quick task"
```

A "turn" is one complete request-response cycle with the AI model.

## Examples

### CI/CD Integration

#### GitHub Actions

```yaml
name: Code Review
on: [pull_request]

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run Cortex Review
        run: |
          cortex exec --auto read-only \
            --git-diff \
            "Review this PR for bugs, security issues, and code quality"
```

#### GitLab CI

```yaml
code-review:
  script:
    - cortex exec --auto read-only -o json "Review the code changes" > review.json
  artifacts:
    paths:
      - review.json
```

### Shell Scripts

#### Automated Bug Fix

```bash
#!/bin/bash
set -e

# Fix all linting errors
cortex exec --auto low \
  --timeout 300 \
  "Fix all ESLint errors in the codebase, commit the changes"
```

#### Batch Processing

```bash
#!/bin/bash
# Process multiple files
for file in src/*.rs; do
  # Quote variable to prevent word splitting and glob expansion
  cortex exec --auto low \
    "Add documentation comments to all public functions in \"$file\""
done
```

#### Conditional Execution

```bash
#!/bin/bash
# Only run if there are changes
if git diff --quiet; then
  echo "No changes to review"
else
  cortex exec --auto read-only --git-diff \
    "Review these changes for issues"
fi
```

### Structured Output

#### JSON Schema Validation

```bash
cortex exec --output-schema '{"type": "object", "properties": {"bugs": {"type": "array"}}}' \
  "List all bugs found as a JSON array"
```

#### Parsing JSON Output

```bash
# Extract specific fields
cortex exec -o json "list dependencies" | jq -r '.response'

# Count items
cortex exec -o json "list TODO comments" | jq '.tool_calls | length'
```

### Multi-Turn Sessions

#### Continue a Session

```bash
# Start a session
SESSION=$(cortex exec -o json "analyze codebase" | jq -r '.session_id')

# Continue the session (quote variable to handle edge cases)
cortex exec -s "$SESSION" "now focus on the auth module"
```

### Context Inclusion

#### Include Files

```bash
# Include specific files
cortex exec --include "src/main.rs" --include "Cargo.toml" \
  "Explain how this application works"

# Include patterns
cortex exec --include "src/**/*.rs" --exclude "**/*_test.rs" \
  "Review the source code"
```

#### Include URLs

```bash
cortex exec --url "https://api.example.com/docs" \
  "How do I authenticate with this API?"
```

#### Include Git Diff

```bash
cortex exec --git-diff "Review my uncommitted changes"
```

### Model Selection

```bash
# Use a specific model
cortex exec -m claude-3-opus "complex reasoning task"

# Use different models for different tasks
cortex exec -m gpt-4-turbo --max-tokens 4000 "generate documentation"
```

### Image Analysis

```bash
# Analyze an image
cortex exec -i screenshot.png "What's shown in this screenshot?"

# Multiple images
cortex exec -i img1.png -i img2.png "Compare these two diagrams"
```

## Best Practices

### 1. Use Appropriate Autonomy Levels

Start with `read-only` and only increase autonomy when necessary:

```bash
# Good: Minimal permissions
cortex exec --auto read-only "review code"

# Good: Appropriate for the task
cortex exec --auto medium "implement feature"

# Bad: Excessive permissions
cortex exec --auto high "read a file"
```

### 2. Set Reasonable Timeouts

Prevent runaway execution with appropriate timeouts:

```bash
# Quick tasks
cortex exec --timeout 60 "format this file"

# Complex tasks
cortex exec --timeout 1800 "refactor entire module"
```

### 3. Use Structured Output for Automation

Parse results programmatically with JSON output:

```bash
result=$(cortex exec -o json "analyze code")
status=$(echo "$result" | jq -r '.status')
```

### 4. Validate Before Production

Test in lower environments first with proper safeguards:

```bash
# Test in staging with timeout and turn limits
cortex exec --auto medium --cwd /staging \
  --timeout 300 --max-turns 20 \
  "test changes"

# Production deployments should include:
# - Explicit timeouts to prevent runaway execution
# - Turn limits for predictable behavior
# - Logging for audit trails
# - Dry-run verification when possible
cortex exec --auto high --cwd /production \
  --timeout 600 --max-turns 50 \
  -o jsonl "deploy" 2>&1 | tee deploy-$(date +%Y%m%d-%H%M%S).log
```

**Production Safety Checklist:**
- [ ] Run dry-run or staging tests first
- [ ] Set explicit `--timeout` values
- [ ] Set explicit `--max-turns` limits
- [ ] Enable logging with `-o jsonl` and `tee`
- [ ] Have rollback procedures ready
- [ ] Monitor execution in real-time when possible

### 5. Log and Monitor

Capture output for debugging:

```bash
cortex exec -o jsonl "task" 2>&1 | tee execution.log
```

## Troubleshooting

### Command Times Out

Increase the timeout or reduce task scope:

```bash
# Increase timeout
cortex exec --timeout 3600 "long task"

# Or break into smaller tasks
cortex exec --timeout 300 "first part"
cortex exec --timeout 300 "second part"
```

### Permission Denied

Check the autonomy level:

```bash
# Upgrade autonomy level
cortex exec --auto medium "task requiring writes"
```

### No Output

Use verbose mode to debug:

```bash
cortex exec -v "task"
```

### Session Not Found

Sessions may expire. Start a new session if continuation fails:

```bash
cortex exec "start fresh with the task"
```
