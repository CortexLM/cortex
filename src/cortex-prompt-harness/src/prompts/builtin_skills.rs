//! Built-in skills for Cortex agent.
//!
//! These skills are embedded in the agent and loaded on-demand when the agent
//! identifies they are relevant to the current task. This reduces context window
//! usage by only including instructions that are actually needed.
//!
//! # Usage
//!
//! ```rust
//! use cortex_prompt_harness::prompts::builtin_skills::{get_builtin_skill, list_builtin_skills};
//!
//! // Get a specific skill by name
//! if let Some(skill) = get_builtin_skill("git") {
//!     println!("Git skill: {}", skill);
//! }
//!
//! // List all available skills
//! for (name, description) in list_builtin_skills() {
//!     println!("{}: {}", name, description);
//! }
//! ```
//!
//! # Skill Format
//!
//! Each skill follows a consistent format with YAML frontmatter containing:
//! - `name`: Unique identifier for the skill
//! - `description`: Brief description of when to use the skill
//! - `version`: Semantic version for tracking changes
//! - `tags`: Categories for organization and discovery

/// List of all available built-in skill names.
pub const BUILTIN_SKILL_NAMES: &[&str] = &[
    "git",
    "code-quality",
    "file-operations",
    "debugging",
    "security",
    "planning",
];

/// Git operations skill - version control best practices.
///
/// Load this skill when performing version control operations such as
/// commits, branches, pull requests, and git history management.
pub const SKILL_GIT: &str = r#"---
name: git
description: Git version control operations, commits, PRs, branches. Load when doing version control tasks.
version: "1.0.0"
tags: [builtin, vcs, git]
---

# Git Operations Skill

## When to Use
Load this skill when performing:
- Creating commits
- Managing branches
- Creating or reviewing pull requests
- Resolving conflicts
- Working with git history

## Git Safety Guidelines

### Before Any Git Operation
```
ALWAYS run 'git status' before other git commands
ALWAYS check changes with 'git diff' before committing
NEVER push without explicit user instruction
NEVER use -i flag (interactive mode not supported)
NEVER update git config without explicit request
```

### Commit Best Practices

#### Message Format
```
<type>(<scope>): <subject>

<body>

<footer>
```

#### Types
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes (formatting, no logic change)
- `refactor`: Code refactoring
- `perf`: Performance improvements
- `test`: Adding or updating tests
- `chore`: Maintenance tasks

#### Good Commit Messages
```
feat(auth): add OAuth2 login support
fix(api): handle null response in user endpoint
docs(readme): update installation instructions
refactor(core): extract validation into separate module
```

#### Bad Commit Messages
```
- "fixed stuff"
- "WIP"
- "updates"
- "misc changes"
```

### Branch Management

#### Naming Convention
```
feature/<description>  - New features
fix/<description>      - Bug fixes
hotfix/<description>   - Urgent production fixes
release/<version>      - Release preparation
```

#### Branch Workflow
1. Always create feature branches from main/master
2. Keep branches focused and short-lived
3. Rebase on main before merging when possible
4. Delete branches after merging

### Pull Request Guidelines

#### Before Creating PR
1. Run `git status` to check uncommitted changes
2. Run `git diff` to review all changes
3. Run `git log` to see recent commits
4. Ensure all tests pass
5. Ensure code follows project conventions

#### PR Creation Process
1. Push branch with `-u` flag: `git push -u origin <branch>`
2. Use `gh pr create` if available
3. Use default branch as base unless specified otherwise
4. Provide clear title and description

#### PR Description Template
```markdown
## Summary
Brief description of changes

## Changes
- Change 1
- Change 2

## Testing
- How was this tested?

## Notes
Any additional context
```

### Conflict Resolution
1. Pull latest changes: `git fetch origin`
2. Identify conflicts: `git status`
3. Resolve conflicts in each file
4. Remove conflict markers: `<<<<<<<`, `=======`, `>>>>>>>`
5. Stage resolved files: `git add <file>`
6. Complete merge/rebase

### History Management
```
PREFER rebase for clean history on feature branches
NEVER force push to shared branches
ALWAYS backup before history rewriting operations
USE git reflog to recover from mistakes
```

### Safe Git Commands
```bash
# Check status
git status

# View changes
git diff
git diff --staged

# View history
git log --oneline -10
git log --graph --oneline --all

# Create branch
git checkout -b <branch-name>

# Switch branch
git checkout <branch-name>

# Stage and commit
git add <files>
git commit -m "type(scope): message"

# Push (first time)
git push -u origin <branch>

# Push (subsequent)
git push
```

### Dangerous Operations (Use with Caution)
```bash
# Force push - can overwrite remote history
git push --force  # Requires explicit user permission

# Reset - can lose commits
git reset --hard  # Only for local cleanup

# Clean - removes untracked files
git clean -fd     # Be sure before running
```
"#;

/// Code quality skill - linting, testing, and style guidelines.
///
/// Load this skill when working on code quality improvements,
/// setting up CI/CD, or ensuring code meets project standards.
pub const SKILL_CODE_QUALITY: &str = r#"---
name: code-quality
description: Code quality standards, linting, testing, and style matching. Load when ensuring code quality.
version: "1.0.0"
tags: [builtin, quality, testing, lint]
---

# Code Quality Skill

## When to Use
Load this skill when:
- Running linters and formatters
- Writing or running tests
- Reviewing code quality
- Setting up CI/CD pipelines
- Ensuring code style consistency

## Core Principles

```
READ first, CODE second.
MATCH the existing patterns.
VERIFY libraries exist before importing.
```

## Style Matching

### Before Writing Code
1. Study existing files in the same module
2. Identify naming conventions (camelCase, snake_case, etc.)
3. Note indentation style (tabs vs spaces, size)
4. Observe import organization patterns
5. Check for project-specific linting rules

### Code Style Rules
```
FOLLOW existing formatting exactly
USE same naming patterns as surrounding code
MATCH bracket style (same line vs new line)
PRESERVE existing organization structure
RESPECT comment style and documentation format
```

## Library Verification

### Before Using Any Library
```bash
# Check package.json for JavaScript/TypeScript
cat package.json | grep "<library>"

# Check requirements.txt for Python
cat requirements.txt | grep "<library>"

# Check Cargo.toml for Rust
cat Cargo.toml | grep "<library>"

# Check go.mod for Go
cat go.mod | grep "<library>"
```

### If Library Not Found
1. Do NOT assume it can be installed
2. Ask user if they want to add the dependency
3. Use alternative approach with existing dependencies
4. Document the missing dependency if proceeding

## Testing Guidelines

### Test Discovery
```bash
# Find test configuration
find . -name "jest.config*" -o -name "pytest.ini" -o -name "*.test.*"

# Find test commands
cat package.json | grep -A5 '"scripts"'
cat Makefile | grep test
```

### Running Tests
```bash
# JavaScript/TypeScript
npm test
npm run test:unit
npm run test:e2e

# Python
pytest
pytest -v
pytest --cov

# Rust
cargo test
cargo test -- --nocapture

# Go
go test ./...
go test -v ./...
```

### Test Quality Rules
```
EVERY feature should have tests
EVERY bug fix should have a regression test
TESTS must be deterministic
TESTS must be independent
MOCK external dependencies
AVOID testing implementation details
```

## Linting and Formatting

### Common Linter Commands
```bash
# JavaScript/TypeScript
npm run lint
npx eslint .
npx prettier --check .

# Python
pylint src/
flake8 src/
black --check src/
mypy src/

# Rust
cargo clippy
cargo fmt --check

# Go
golint ./...
go fmt ./...
go vet ./...
```

### Fixing Lint Issues
1. Run linter in check mode first
2. Review reported issues
3. Fix issues in order of severity
4. Re-run linter to confirm
5. Run tests to ensure nothing broke

## Quality Checkpoints

### Before Action
```
├── Requirement understood?
├── Relevant files read?
├── Side effects mapped?
├── Right tool selected?
└── Following existing patterns?
```

### After Action
```
├── Change applied correctly?
├── No syntax errors?
├── Functionality preserved?
└── Style consistent?
```

### Before Completion
```
├── All requirements met?
├── Tests passing?
├── No errors in system messages?
├── Summary ready?
└── Plan updated?
```

## Code Review Checklist
```
□ Code compiles/runs without errors
□ All tests pass
□ No new linting warnings
□ Follows project coding standards
□ Error handling is appropriate
□ No hardcoded values (use config)
□ No sensitive data exposed
□ Documentation updated if needed
□ Changes are focused and minimal
```
"#;

/// File operations skill - safe file reading and writing practices.
///
/// Load this skill when performing file system operations to ensure
/// safe and recoverable file modifications.
pub const SKILL_FILE_OPERATIONS: &str = r#"---
name: file-operations
description: Safe file operations, read-before-write patterns, and rollback strategies. Load when modifying files.
version: "1.0.0"
tags: [builtin, files, safety]
---

# File Operations Skill

## When to Use
Load this skill when:
- Creating new files
- Modifying existing files
- Moving or renaming files
- Deleting files
- Working with file permissions

## Core Rules

```
PREFER Patch over Write for existing files
ALWAYS Read before Patch
THINK rollback before every change
```

## Read Before Write/Patch

### Why Read First
1. Understand current file state
2. Identify patterns and conventions
3. Verify target location exists
4. Check for potential conflicts
5. Plan minimal necessary changes

### Read Patterns
```bash
# Read entire file (for small files)
Read <filepath>

# Read specific section
Read <filepath> --lines 50-100

# Check if file exists
ls -la <filepath>
```

## Patch vs Write

### When to Use Patch
- File already exists
- Making targeted changes
- Preserving unchanged content
- Multiple edits to same file

### When to Use Write
- Creating new files
- Complete file replacement (rare)
- Generated content

### Patch Best Practices
```
1. Read file first to understand context
2. Identify exact lines to change
3. Make minimal necessary edits
4. Preserve surrounding code style
5. Verify change applied correctly
```

## Rollback Strategy

### Before Making Changes
1. Understand what will change
2. Consider how to undo if needed
3. For risky operations, note current state

### Rollback Options
```
Git-based:
- git checkout -- <file>     # Discard local changes
- git restore <file>         # Restore from staging
- git stash                  # Temporarily save changes

Manual:
- Keep backup of original content
- Document pre-change state
- Use version control properly
```

### When to Rollback
- Change causes test failures
- Unexpected side effects
- User requests undo
- Change doesn't meet requirements

## File Safety Rules

### Never Do
```
- Write without reading first
- Delete without confirmation
- Modify system files
- Change file permissions without reason
- Create files outside project directory
```

### Always Do
```
- Verify paths are correct
- Use absolute paths when possible
- Check file exists before patching
- Respect .gitignore patterns
- Preserve file encoding
```

## Directory Operations

### Creating Directories
```bash
# Create with parents
mkdir -p path/to/new/dir

# Verify creation
ls -la path/to/new/
```

### Moving/Renaming
```bash
# Check destination doesn't exist
ls destination/path

# Move with safety
mv -i source destination  # Interactive mode
```

### Deleting
```
NEVER delete without explicit request
ALWAYS confirm path before deletion
PREFER moving to trash over rm
LIST contents before removing directories
```

## Large File Handling

### For Large Files
1. Read specific sections, not entire file
2. Use streaming for processing
3. Consider memory constraints
4. Break into smaller chunks

### Partial File Reading
```bash
# Read specific lines
Read <file> --lines 1-50

# Search for patterns
Grep <pattern> <file>
```

## Special Files

### Configuration Files
```
BACKUP before modifying
VALIDATE format after changes
TEST that config loads properly
```

### Binary Files
```
NEVER modify binary files directly
USE appropriate tools for each format
VERIFY integrity after operations
```

### Lock Files
```
DO NOT manually edit lock files
USE package manager commands
COMMIT lock files to version control
```
"#;

/// Debugging skill - systematic error handling and recovery.
///
/// Load this skill when encountering errors, debugging issues,
/// or implementing error recovery strategies.
pub const SKILL_DEBUGGING: &str = r#"---
name: debugging
description: Systematic debugging, error handling, and failure recovery. Load when troubleshooting issues.
version: "1.0.0"
tags: [builtin, debugging, errors]
---

# Debugging Skill

## When to Use
Load this skill when:
- Encountering errors or exceptions
- Tests are failing
- Unexpected behavior occurs
- Need to implement error handling
- Recovery from failed operations

## Failure Protocol

When something breaks, escalate systematically through tiers:

### TIER 1: RETRY
```
├── Read the error message carefully
├── Check paths, typos, syntax
├── Try slight variations
└── Max 3 attempts → escalate to Tier 2
```

#### Common Quick Fixes
- Check file paths are correct
- Verify command syntax
- Ensure required files exist
- Check for missing semicolons/brackets
- Verify environment is correct

### TIER 2: PIVOT
```
├── Undo what broke things
├── Research alternatives
├── Try different approach
└── Consult docs via Fetch/WebQuery
```

#### Pivot Strategies
1. Roll back to last working state
2. Search for similar issues online
3. Check documentation for correct usage
4. Try alternative implementation
5. Isolate the problem component

### TIER 3: DECOMPOSE
```
├── Break into smaller pieces
├── Isolate the failing part
├── Solve pieces independently
└── Delegate if needed
```

#### Decomposition Process
1. Identify the smallest failing unit
2. Create minimal reproduction case
3. Test components in isolation
4. Fix one issue at a time
5. Rebuild from working pieces

### TIER 4: GRACEFUL EXIT
```
├── Document what was tried
├── Explain the blocker
├── Suggest workarounds
├── Complete what's possible
└── Leave code in working state
```

#### Graceful Exit Requirements
- Code must compile/run
- Tests that were passing still pass
- Clear documentation of the issue
- Suggested next steps
- No broken functionality left behind

## Error Analysis

### Reading Error Messages
1. Identify error type (syntax, runtime, logic)
2. Find line number and file
3. Understand error description
4. Check stack trace for context
5. Look for root cause, not symptoms

### Common Error Types

#### Syntax Errors
```
- Missing brackets/parentheses
- Incorrect indentation
- Missing semicolons
- Typos in keywords
```

#### Runtime Errors
```
- Null/undefined references
- Type mismatches
- Out of bounds access
- File not found
- Permission denied
```

#### Logic Errors
```
- Incorrect conditions
- Off-by-one errors
- Wrong variable used
- Missing edge cases
- Incorrect algorithms
```

## Debugging Techniques

### Print Debugging
```
Add strategic print/log statements:
1. Function entry/exit points
2. Before/after suspicious code
3. Variable values at key points
4. Loop iteration values
```

### Binary Search Debugging
1. Find last known working state
2. Find first known broken state
3. Test midpoint between them
4. Narrow down to exact change

### Rubber Duck Debugging
1. Explain the code line by line
2. Describe what should happen
3. Describe what actually happens
4. The discrepancy reveals the bug

## Error Recovery Strategies

### Transient Errors
```
- Network timeouts → Retry with backoff
- Rate limits → Wait and retry
- Temporary file locks → Wait and retry
```

### Permanent Errors
```
- Missing dependencies → Install or use alternative
- Invalid input → Validate and report
- Permission denied → Request access or skip
```

### Partial Success
```
- Log what succeeded
- Save partial results
- Report what failed
- Provide recovery options
```

## Hard Rule

**Never leave the codebase broken. Rollback if needed.**

### Before Stopping
```
□ Code compiles
□ Existing tests pass
□ No new syntax errors
□ Changes are documented
□ User understands the state
```

## Debugging Tools

### Language-Specific
```bash
# JavaScript/TypeScript
console.log(), debugger statement
node --inspect

# Python
print(), breakpoint()
python -m pdb

# Rust
dbg!(), println!()
cargo test -- --nocapture

# Go
fmt.Printf(), log.Printf()
delve debugger
```

### General Tools
```bash
# Check logs
tail -f /var/log/app.log

# Monitor processes
ps aux | grep <process>
top

# Check disk/memory
df -h
free -m
```
"#;

/// Security skill - secure coding and data protection practices.
///
/// Load this skill when handling sensitive data, implementing
/// authentication, or reviewing code for security issues.
pub const SKILL_SECURITY: &str = r#"---
name: security
description: Secure coding practices, secrets handling, and input validation. Load when handling sensitive data.
version: "1.0.0"
tags: [builtin, security, secrets]
---

# Security Skill

## When to Use
Load this skill when:
- Handling passwords, API keys, or tokens
- Implementing authentication/authorization
- Processing user input
- Reviewing code for security issues
- Setting up secure configurations

## Absolute Rules

```
NEVER expose: keys, secrets, tokens, passwords
NEVER log sensitive data, even in debug
ALWAYS sanitize inputs
ALWAYS use secure defaults
```

## Secrets Handling

### Never Do
```
- Hardcode secrets in source code
- Commit secrets to version control
- Log secrets even in debug mode
- Include secrets in error messages
- Pass secrets in URLs
- Store secrets in plain text files
```

### Always Do
```
- Use environment variables
- Use secrets management systems
- Encrypt secrets at rest
- Rotate secrets regularly
- Use least privilege access
- Audit secret access
```

### Secret Patterns to Avoid
```javascript
// NEVER DO THIS
const API_KEY = "sk-1234567890abcdef";
const PASSWORD = "secret123";
const TOKEN = "ghp_xxxxxxxxxxxxxxxxxxxx";

// DO THIS INSTEAD
const API_KEY = process.env.API_KEY;
const PASSWORD = process.env.DB_PASSWORD;
const TOKEN = process.env.GITHUB_TOKEN;
```

## Input Validation

### Validate All Input
```
- User form data
- API request bodies
- URL parameters
- File uploads
- Headers and cookies
```

### Validation Rules
```
- Whitelist allowed characters
- Enforce length limits
- Validate data types
- Check for malicious patterns
- Sanitize before use
```

### Common Attacks to Prevent
```
SQL Injection:
- Use parameterized queries
- Never concatenate user input into SQL

XSS (Cross-Site Scripting):
- Escape HTML output
- Use Content Security Policy
- Sanitize user-generated content

Command Injection:
- Never pass user input to shell
- Use safe APIs instead of exec()
- Whitelist allowed commands

Path Traversal:
- Validate file paths
- Use safe path joining
- Restrict to allowed directories
```

## Logging Safety

### Never Log
```
- Passwords
- API keys
- Tokens
- Credit card numbers
- Social security numbers
- Personal health information
- Encryption keys
```

### Safe Logging Practices
```python
# WRONG
logger.info(f"User login: {username}, password: {password}")

# RIGHT
logger.info(f"User login attempt: {username}")

# WRONG
logger.debug(f"API response: {response}")  # May contain secrets

# RIGHT
logger.debug(f"API response status: {response.status_code}")
```

## Secure Defaults

### Configuration
```
- Deny by default, allow explicitly
- Disable unnecessary features
- Use strong encryption algorithms
- Set secure cookie flags
- Enable HTTPS only
```

### Authentication
```
- Require strong passwords
- Implement rate limiting
- Use secure session management
- Enable MFA where possible
- Lock accounts after failed attempts
```

### File Permissions
```bash
# Restrict sensitive files
chmod 600 private_key.pem
chmod 700 ~/.ssh

# Never do
chmod 777 anything
```

## Code Review Security Checklist

```
□ No hardcoded secrets
□ All input is validated
□ Output is properly escaped
□ SQL uses parameterized queries
□ File paths are validated
□ Error messages don't leak info
□ Logging doesn't include secrets
□ Dependencies are up to date
□ HTTPS is enforced
□ Authentication is implemented correctly
```

## Dependency Security

### Before Adding Dependencies
```
- Check for known vulnerabilities
- Review dependency reputation
- Verify license compatibility
- Check maintenance status
- Audit transitive dependencies
```

### Vulnerability Scanning
```bash
# JavaScript
npm audit
npm audit fix

# Python
pip-audit
safety check

# Rust
cargo audit

# General
snyk test
```

## Secure Communication

### HTTPS/TLS
```
- Always use HTTPS in production
- Verify SSL certificates
- Use TLS 1.2 or higher
- Configure secure cipher suites
```

### API Security
```
- Use authentication tokens
- Implement rate limiting
- Validate content types
- Log access for auditing
- Use CORS appropriately
```

## Incident Response

### If Secrets Are Exposed
1. Revoke the compromised secret immediately
2. Generate new secret
3. Update all systems using the secret
4. Review logs for unauthorized access
5. Document the incident
6. Implement prevention measures
"#;

/// Planning skill - task decomposition and cognitive architecture.
///
/// Load this skill when working on complex tasks that require
/// structured planning and phase-based execution.
pub const SKILL_PLANNING: &str = r#"---
name: planning
description: Task decomposition, cognitive architecture, and systematic execution. Load for complex multi-step tasks.
version: "1.0.0"
tags: [builtin, planning, architecture]
---

# Planning Skill

## When to Use
Load this skill when:
- Starting complex multi-step tasks
- Breaking down large features
- Coordinating multiple changes
- Deciding on delegation
- Need structured execution approach

## Cognitive Architecture

Every task flows through five phases. No shortcuts.

```
┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐
│ RECON   │───▶│ DESIGN  │───▶│ BUILD   │───▶│ VERIFY  │───▶│ CLOSE   │
└─────────┘    └─────────┘    └─────────┘    └─────────┘    └─────────┘
     │                              ▲              │
     │                              └──────────────┘
     │                                 (on failure)
     └─────────────────────────────────────────────────────────────────▶
                              (new task triggers new cycle)
```

## Phase 1: RECON

> Understand before touching anything.

### What to Do
- Scan project structure, find README or docs
- Identify patterns, conventions, dependencies
- Map what exists before planning what to add
- Understand the scope and constraints

### RECON Checklist
```
□ Project structure understood
□ Relevant files identified
□ Existing patterns documented
□ Dependencies mapped
□ Constraints known
□ Success criteria defined
```

### RECON Questions
1. What does the codebase look like?
2. What conventions are used?
3. What already exists that I can reuse?
4. What are the risks?
5. What's the minimal change needed?

## Phase 2: DESIGN

> Plan the attack. Break it down.

### What to Do
- Decompose into atomic steps
- Identify risks and dependencies
- Decide what to delegate to sub-agents
- Create a clear execution plan

### Task Decomposition

#### Good Decomposition
```
✓ Each step is independently testable
✓ Steps have clear inputs and outputs
✓ Dependencies between steps are explicit
✓ Steps can be parallelized where possible
✓ Each step has clear success criteria
```

#### Bad Decomposition
```
✗ Steps are vague ("implement feature")
✗ Steps have hidden dependencies
✗ Steps can't be verified independently
✗ Too many steps (more than 10)
✗ Too few steps for complex tasks
```

### Delegation Decisions

#### Delegate When
- Task is well-defined and isolated
- Sub-agent has specialized expertise
- Parallel execution is beneficial
- Task is low-risk and recoverable

#### Don't Delegate When
- Task requires broader context
- Changes affect many files
- High coordination needed
- Risk of inconsistent changes

## Phase 3: BUILD

> Execute with precision. One change at a time.

### What to Do
- Implement step by step
- Respect existing code style religiously
- Verify each change before the next

### Build Rules
```
ONE change at a time
TEST after each change
COMMIT logical units
DOCUMENT complex logic
ROLLBACK on failure
```

### Build Checklist (per step)
```
□ Read relevant files first
□ Understand what to change
□ Make minimal change
□ Verify change works
□ Check style matches
□ Move to next step
```

## Phase 4: VERIFY

> Trust nothing. Test everything.

### What to Do
- Run linters, type checkers, tests
- Confirm requirements are met
- Check for regressions

### Verification Commands
```bash
# Find and run project tests
npm test / pytest / cargo test / go test

# Run linters
npm run lint / pylint / cargo clippy

# Type check
tsc --noEmit / mypy / cargo check
```

### Verification Checklist
```
□ All tests pass
□ No new linting errors
□ No type errors
□ Requirements met
□ No regressions
□ Edge cases handled
```

## Phase 5: CLOSE

> Wrap it up clean.

### What to Do
- Summarize in 1-4 sentences
- Mark all tasks complete in Plan
- Note any caveats or follow-ups

### Close Checklist
```
□ All steps marked complete
□ Summary written
□ Caveats documented
□ Follow-ups noted
□ Code is clean
```

## Planning Templates

### Feature Implementation
```
1. RECON: Understand existing code
2. DESIGN: Plan changes and new files
3. BUILD:
   a. Create new files/modules
   b. Implement core logic
   c. Add tests
   d. Update related files
4. VERIFY: Run all tests
5. CLOSE: Summarize changes
```

### Bug Fix
```
1. RECON: Reproduce and understand bug
2. DESIGN: Identify root cause
3. BUILD:
   a. Write failing test
   b. Fix the bug
   c. Verify test passes
4. VERIFY: Ensure no regressions
5. CLOSE: Document fix
```

### Refactoring
```
1. RECON: Map current structure
2. DESIGN: Plan new structure
3. BUILD:
   a. Create new structure
   b. Migrate piece by piece
   c. Update imports/references
   d. Remove old code
4. VERIFY: All tests still pass
5. CLOSE: Document changes
```

## Task Priority Rules

```
CRITICAL: Blocking issues, security vulnerabilities
HIGH: Core functionality, user-facing bugs
MEDIUM: Improvements, non-critical bugs
LOW: Nice-to-have, optimization, cleanup
```

## Estimation Guidelines

```
XS (< 30 min): Single file change, simple fix
S  (30-60 min): Multiple related changes
M  (1-2 hours): Feature spanning multiple files
L  (2-4 hours): Complex feature, new module
XL (4+ hours): Major refactor, new system
```
"#;

/// Retrieve a built-in skill by name.
///
/// # Arguments
///
/// * `name` - The name of the skill to retrieve (case-insensitive)
///
/// # Returns
///
/// Returns `Some(&str)` with the skill content if found, or `None` if the skill
/// does not exist.
///
/// # Example
///
/// ```rust
/// use cortex_prompt_harness::prompts::builtin_skills::get_builtin_skill;
///
/// if let Some(skill) = get_builtin_skill("git") {
///     assert!(skill.contains("Git Operations Skill"));
/// }
///
/// assert!(get_builtin_skill("nonexistent").is_none());
/// ```
pub fn get_builtin_skill(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "git" => Some(SKILL_GIT),
        "code-quality" => Some(SKILL_CODE_QUALITY),
        "file-operations" => Some(SKILL_FILE_OPERATIONS),
        "debugging" => Some(SKILL_DEBUGGING),
        "security" => Some(SKILL_SECURITY),
        "planning" => Some(SKILL_PLANNING),
        _ => None,
    }
}

/// List all built-in skills with their names and descriptions.
///
/// # Returns
///
/// Returns a vector of tuples containing (name, description) for each
/// available built-in skill.
///
/// # Example
///
/// ```rust
/// use cortex_prompt_harness::prompts::builtin_skills::list_builtin_skills;
///
/// let skills = list_builtin_skills();
/// assert_eq!(skills.len(), 6);
///
/// for (name, description) in skills {
///     println!("{}: {}", name, description);
/// }
/// ```
pub fn list_builtin_skills() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "git",
            "Git version control operations, commits, PRs, branches. Load when doing version control tasks.",
        ),
        (
            "code-quality",
            "Code quality standards, linting, testing, and style matching. Load when ensuring code quality.",
        ),
        (
            "file-operations",
            "Safe file operations, read-before-write patterns, and rollback strategies. Load when modifying files.",
        ),
        (
            "debugging",
            "Systematic debugging, error handling, and failure recovery. Load when troubleshooting issues.",
        ),
        (
            "security",
            "Secure coding practices, secrets handling, and input validation. Load when handling sensitive data.",
        ),
        (
            "planning",
            "Task decomposition, cognitive architecture, and systematic execution. Load for complex multi-step tasks.",
        ),
    ]
}

/// Get the total count of built-in skills.
///
/// # Returns
///
/// Returns the number of available built-in skills.
///
/// # Example
///
/// ```rust
/// use cortex_prompt_harness::prompts::builtin_skills::builtin_skill_count;
///
/// assert_eq!(builtin_skill_count(), 6);
/// ```
pub fn builtin_skill_count() -> usize {
    BUILTIN_SKILL_NAMES.len()
}

/// Check if a skill name refers to a built-in skill.
///
/// # Arguments
///
/// * `name` - The name to check (case-insensitive)
///
/// # Returns
///
/// Returns `true` if the name refers to a built-in skill, `false` otherwise.
///
/// # Example
///
/// ```rust
/// use cortex_prompt_harness::prompts::builtin_skills::is_builtin_skill;
///
/// assert!(is_builtin_skill("git"));
/// assert!(is_builtin_skill("Git")); // case-insensitive
/// assert!(!is_builtin_skill("custom-skill"));
/// ```
pub fn is_builtin_skill(name: &str) -> bool {
    get_builtin_skill(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_skill_names_count() {
        assert_eq!(BUILTIN_SKILL_NAMES.len(), 6);
    }

    #[test]
    fn test_builtin_skill_names_contents() {
        assert!(BUILTIN_SKILL_NAMES.contains(&"git"));
        assert!(BUILTIN_SKILL_NAMES.contains(&"code-quality"));
        assert!(BUILTIN_SKILL_NAMES.contains(&"file-operations"));
        assert!(BUILTIN_SKILL_NAMES.contains(&"debugging"));
        assert!(BUILTIN_SKILL_NAMES.contains(&"security"));
        assert!(BUILTIN_SKILL_NAMES.contains(&"planning"));
    }

    #[test]
    fn test_get_builtin_skill_git() {
        let skill = get_builtin_skill("git");
        assert!(skill.is_some());
        let content = skill.unwrap();
        assert!(content.contains("name: git"));
        assert!(content.contains("Git Operations Skill"));
        assert!(content.contains("Commit Best Practices"));
        assert!(content.contains("Branch Management"));
        assert!(content.contains("Pull Request Guidelines"));
    }

    #[test]
    fn test_get_builtin_skill_code_quality() {
        let skill = get_builtin_skill("code-quality");
        assert!(skill.is_some());
        let content = skill.unwrap();
        assert!(content.contains("name: code-quality"));
        assert!(content.contains("Code Quality Skill"));
        assert!(content.contains("Style Matching"));
        assert!(content.contains("Library Verification"));
        assert!(content.contains("Testing Guidelines"));
    }

    #[test]
    fn test_get_builtin_skill_file_operations() {
        let skill = get_builtin_skill("file-operations");
        assert!(skill.is_some());
        let content = skill.unwrap();
        assert!(content.contains("name: file-operations"));
        assert!(content.contains("File Operations Skill"));
        assert!(content.contains("Read Before Write"));
        assert!(content.contains("Patch vs Write"));
        assert!(content.contains("Rollback Strategy"));
    }

    #[test]
    fn test_get_builtin_skill_debugging() {
        let skill = get_builtin_skill("debugging");
        assert!(skill.is_some());
        let content = skill.unwrap();
        assert!(content.contains("name: debugging"));
        assert!(content.contains("Debugging Skill"));
        assert!(content.contains("TIER 1: RETRY"));
        assert!(content.contains("TIER 2: PIVOT"));
        assert!(content.contains("TIER 3: DECOMPOSE"));
        assert!(content.contains("TIER 4: GRACEFUL EXIT"));
    }

    #[test]
    fn test_get_builtin_skill_security() {
        let skill = get_builtin_skill("security");
        assert!(skill.is_some());
        let content = skill.unwrap();
        assert!(content.contains("name: security"));
        assert!(content.contains("Security Skill"));
        assert!(content.contains("Secrets Handling"));
        assert!(content.contains("Input Validation"));
        assert!(content.contains("Logging Safety"));
    }

    #[test]
    fn test_get_builtin_skill_planning() {
        let skill = get_builtin_skill("planning");
        assert!(skill.is_some());
        let content = skill.unwrap();
        assert!(content.contains("name: planning"));
        assert!(content.contains("Planning Skill"));
        assert!(content.contains("Cognitive Architecture"));
        assert!(content.contains("RECON"));
        assert!(content.contains("DESIGN"));
        assert!(content.contains("BUILD"));
        assert!(content.contains("VERIFY"));
        assert!(content.contains("CLOSE"));
    }

    #[test]
    fn test_get_builtin_skill_case_insensitive() {
        assert!(get_builtin_skill("git").is_some());
        assert!(get_builtin_skill("GIT").is_some());
        assert!(get_builtin_skill("Git").is_some());
        assert!(get_builtin_skill("CODE-QUALITY").is_some());
        assert!(get_builtin_skill("Code-Quality").is_some());
    }

    #[test]
    fn test_get_builtin_skill_nonexistent() {
        assert!(get_builtin_skill("nonexistent").is_none());
        assert!(get_builtin_skill("").is_none());
        assert!(get_builtin_skill("random-skill").is_none());
    }

    #[test]
    fn test_list_builtin_skills() {
        let skills = list_builtin_skills();
        assert_eq!(skills.len(), 6);

        let names: Vec<&str> = skills.iter().map(|(name, _)| *name).collect();
        assert!(names.contains(&"git"));
        assert!(names.contains(&"code-quality"));
        assert!(names.contains(&"file-operations"));
        assert!(names.contains(&"debugging"));
        assert!(names.contains(&"security"));
        assert!(names.contains(&"planning"));

        // Check all descriptions are non-empty
        for (_, description) in &skills {
            assert!(!description.is_empty());
        }
    }

    #[test]
    fn test_builtin_skill_count() {
        assert_eq!(builtin_skill_count(), 6);
        assert_eq!(builtin_skill_count(), BUILTIN_SKILL_NAMES.len());
    }

    #[test]
    fn test_is_builtin_skill() {
        assert!(is_builtin_skill("git"));
        assert!(is_builtin_skill("GIT")); // case-insensitive
        assert!(is_builtin_skill("code-quality"));
        assert!(is_builtin_skill("file-operations"));
        assert!(is_builtin_skill("debugging"));
        assert!(is_builtin_skill("security"));
        assert!(is_builtin_skill("planning"));

        assert!(!is_builtin_skill("nonexistent"));
        assert!(!is_builtin_skill(""));
        assert!(!is_builtin_skill("custom"));
    }

    #[test]
    fn test_skill_yaml_frontmatter_format() {
        // All skills should have proper YAML frontmatter
        let skills = [
            SKILL_GIT,
            SKILL_CODE_QUALITY,
            SKILL_FILE_OPERATIONS,
            SKILL_DEBUGGING,
            SKILL_SECURITY,
            SKILL_PLANNING,
        ];

        for skill in skills {
            assert!(skill.starts_with("---\n"), "Skill should start with YAML frontmatter");
            assert!(skill.contains("\n---\n"), "Skill should have frontmatter end marker");
            assert!(skill.contains("name:"), "Skill should have name field");
            assert!(skill.contains("description:"), "Skill should have description field");
            assert!(skill.contains("version:"), "Skill should have version field");
            assert!(skill.contains("tags:"), "Skill should have tags field");
        }
    }

    #[test]
    fn test_skill_content_sections() {
        // All skills should have "When to Use" section
        let skills = [
            SKILL_GIT,
            SKILL_CODE_QUALITY,
            SKILL_FILE_OPERATIONS,
            SKILL_DEBUGGING,
            SKILL_SECURITY,
            SKILL_PLANNING,
        ];

        for skill in skills {
            assert!(
                skill.contains("## When to Use"),
                "Skill should have 'When to Use' section"
            );
        }
    }

    #[test]
    fn test_skill_versions_are_valid() {
        let skills = list_builtin_skills();
        for (name, _) in skills {
            let skill = get_builtin_skill(name).unwrap();
            assert!(
                skill.contains("version: \"1.0.0\""),
                "Skill {} should have version 1.0.0",
                name
            );
        }
    }

    #[test]
    fn test_skill_tags_include_builtin() {
        let skills = list_builtin_skills();
        for (name, _) in skills {
            let skill = get_builtin_skill(name).unwrap();
            assert!(
                skill.contains("builtin"),
                "Skill {} should have 'builtin' tag",
                name
            );
        }
    }
}
