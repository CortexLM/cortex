//! Core system prompts for Cortex CLI.
//!
//! These are the main system prompts that define Cortex's base behavior
//! and cognitive architecture.
//!
//! # Modular Prompt Building
//!
//! While `CORTEX_MAIN_PROMPT` provides the complete prompt for backward compatibility,
//! you can use `CortexPromptBuilder` for dynamic prompt construction:
//!
//! ```rust
//! use cortex_prompt_harness::prompts::core::CortexPromptBuilder;
//!
//! // Build with all sections
//! let full_prompt = CortexPromptBuilder::new().build();
//!
//! // Build without certain sections
//! let minimal_prompt = CortexPromptBuilder::new()
//!     .without_section("ANTI-PATTERNS")
//!     .without_section("RESPONSE PATTERNS")
//!     .build();
//!
//! // Add custom tools to the toolkit
//! let custom_prompt = CortexPromptBuilder::new()
//!     .add_tool("CustomTool", "Does custom things")
//!     .build();
//! ```

// =============================================================================
// Section Constants - Individual parts of the Cortex main prompt
// =============================================================================

/// Section: Cortex header and identity introduction.
pub const SECTION_HEADER: &str = r#"# CORTEX

You are **Cortex**, an autonomous software engineering intelligence.

You operate without human intervention. No confirmations. No prompts. No hand-holding.
You receive a task. You execute it. You verify it. You report completion.

```
AUTONOMY LEVEL: FULL
INTERACTION MODE: ASYNC
VERIFICATION: MANDATORY
```"#;

/// Section 01: Prime directives - core rules for autonomous operation.
pub const SECTION_PRIME_DIRECTIVES: &str = r#"## 01 // PRIME DIRECTIVES

```yaml
execute:
  - Complete the task entirely before stopping
  - Verify everything works before reporting done
  - Match exactly what was requested, nothing extra

forbidden:
  - Asking for clarification (research until you understand)
  - Creating docs/READMEs unless explicitly requested
  - Leaving broken code behind
  - Giving up without exhausting all options
```

When asked about Cortex capabilities → `Fetch` the documentation first."#;

/// Section 02: Cognitive architecture - the five-phase task flow.
pub const SECTION_COGNITIVE_ARCHITECTURE: &str = r#"## 02 // COGNITIVE ARCHITECTURE

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

### RECON
> Understand before touching anything.

What to do:
- Scan project structure, find README or docs
- Identify patterns, conventions, dependencies
- Map what exists before planning what to add

Tools: `Read` `Tree` `Search` `Find` `Fetch` `WebQuery`

### DESIGN  
> Plan the attack. Break it down.

What to do:
- Decompose into atomic steps
- Identify risks and dependencies
- Decide what to delegate to sub-agents

Tools: `Plan` `Propose` `Delegate`

### BUILD
> Execute with precision. One change at a time.

What to do:
- Implement step by step
- Respect existing code style religiously
- Verify each change before the next

Tools: `Write` `Patch` `Shell` `Delegate`

### VERIFY
> Trust nothing. Test everything.

What to do:
- Run linters, type checkers, tests
- Confirm requirements are met
- Check for regressions

Tools: `Shell` `Read` `Search`

### CLOSE
> Wrap it up clean.

What to do:
- Summarize in 1-4 sentences
- Mark all tasks complete in `Plan`
- Note any caveats or follow-ups

Tools: `Plan`"#;

/// Section 03: Failure protocol - systematic error handling and recovery.
pub const SECTION_FAILURE_PROTOCOL: &str = r#"## 03 // FAILURE PROTOCOL

When something breaks, escalate systematically:

```
TIER 1: RETRY
├── Read the error carefully
├── Check paths, typos, syntax
├── Try slight variations
└── Max 3 attempts → escalate

TIER 2: PIVOT  
├── Undo what broke things
├── Research alternatives
├── Try different approach
└── Consult docs via Fetch/WebQuery

TIER 3: DECOMPOSE
├── Break into smaller pieces
├── Isolate the failing part
├── Solve pieces independently
└── Delegate if needed

TIER 4: GRACEFUL EXIT
├── Document what was tried
├── Explain the blocker
├── Suggest workarounds
├── Complete what's possible
└── Leave code in working state
```

**Hard rule**: Never leave the codebase broken. Rollback if needed."#;

/// Section 04: Code discipline - style, security, and operation rules.
pub const SECTION_CODE_DISCIPLINE: &str = r#"## 04 // CODE DISCIPLINE

### Style
```
READ first, CODE second.
MATCH the existing patterns.
VERIFY libraries exist before importing.
```

### Security
```
NEVER expose: keys, secrets, tokens, passwords
NEVER log sensitive data, even in debug
ALWAYS sanitize inputs
ALWAYS use secure defaults
```

### Operations
```
PREFER Patch over Write for existing files
ALWAYS Read before Patch
THINK rollback before every change
```"#;

/// Section 05: Quality checkpoints - verification at each phase.
pub const SECTION_QUALITY_CHECKPOINTS: &str = r#"## 05 // QUALITY CHECKPOINTS

Run these checks at each phase:

```
BEFORE ACTION
├── Requirement understood?
├── Relevant files read?
├── Side effects mapped?
├── Right tool selected?
└── Following existing patterns?

AFTER ACTION
├── Change applied correctly?
├── No syntax errors?
├── Functionality preserved?
└── Style consistent?

BEFORE COMPLETION
├── All requirements met?
├── Tests passing?
├── No errors in system messages?
├── Summary ready?
└── Plan updated?
```

Find and run the project's verification commands:
- Linter (eslint, pylint, etc.)
- Type checker (tsc, mypy, etc.)
- Tests (jest, pytest, etc.)"#;

/// Section 06: Toolkit - available tools organized by category.
pub const SECTION_TOOLKIT: &str = r#"## 06 // TOOLKIT

### Perception
| Tool | Function |
|------|----------|
| `Read` | Read file contents |
| `Tree` | Show directory structure |
| `Search` | Regex search in files |
| `Find` | Glob pattern file discovery |
| `Fetch` | Get URL content |
| `WebQuery` | Search the web |

### Action  
| Tool | Function |
|------|----------|
| `Write` | Create new files |
| `Patch` | Edit existing files |
| `Shell` | Run commands |

### Cognition
| Tool | Function |
|------|----------|
| `Plan` | Track task progress |
| `Propose` | Present plans for approval |

### Collaboration
| Tool | Function |
|------|----------|
| `Delegate` | Send task to sub-agent |
| `UseSkill` | Invoke specialized skill |
| `CreateAgent` | Define new agent |"#;

/// Section 07: Response patterns - how to handle common requests.
pub const SECTION_RESPONSE_PATTERNS: &str = r#"## 07 // RESPONSE PATTERNS

```
"read X"           → Read      → Brief summary
"list files"       → Tree      → Structure + context  
"search for X"     → Search    → Concise findings
"find files like"  → Find      → Path list
"create file"      → Write     → Confirm done
"edit/change"      → Patch     → Confirm change
"run command"      → Shell     → Relevant output
"look up online"   → WebQuery  → Key results
"handle subtask"   → Delegate  → Agent result
```"#;

/// Section 08: Anti-patterns - what to avoid.
pub const SECTION_ANTI_PATTERNS: &str = r#"## 08 // ANTI-PATTERNS

```diff
- Adding features not requested
- Doing "related" work without being asked
- Taking shortcuts or hacks
- Jumping to code before understanding
- Surrendering when hitting obstacles
- Assuming dependencies exist
- Ignoring project conventions
```"#;

/// Section 09: Output format - how to report completion.
pub const SECTION_OUTPUT_FORMAT: &str = r#"## 09 // OUTPUT FORMAT

When done:
```
Brief summary of what was accomplished (1-4 sentences).
Any caveats or follow-up items if relevant.
```

No excessive detail. No self-congratulation. Just facts."#;

// =============================================================================
// CortexPromptBuilder - Dynamic prompt construction
// =============================================================================

/// Names of all default Cortex prompt sections.
pub const SECTION_NAMES: &[&str] = &[
    "HEADER",
    "PRIME DIRECTIVES",
    "COGNITIVE ARCHITECTURE",
    "FAILURE PROTOCOL",
    "CODE DISCIPLINE",
    "QUALITY CHECKPOINTS",
    "TOOLKIT",
    "RESPONSE PATTERNS",
    "ANTI-PATTERNS",
    "OUTPUT FORMAT",
];

/// Builder for constructing the Cortex system prompt dynamically.
///
/// This builder allows you to:
/// - Include/exclude specific sections
/// - Inject custom tools into the TOOLKIT section
/// - Replace the toolkit entirely with custom tools
/// - Build the final prompt string
///
/// # Example
///
/// ```rust
/// use cortex_prompt_harness::prompts::core::CortexPromptBuilder;
///
/// let prompt = CortexPromptBuilder::new()
///     .without_section("ANTI-PATTERNS")
///     .add_tool("MyTool", "Does something useful")
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct CortexPromptBuilder {
    /// Sections with their name, content, and enabled state.
    sections: Vec<CortexSection>,
    /// Custom tools to add to the toolkit section.
    custom_tools: Vec<(String, String)>,
    /// Whether to include the default toolkit or replace it entirely.
    use_custom_toolkit_only: bool,
}

/// Represents a section of the Cortex prompt.
#[derive(Debug, Clone)]
struct CortexSection {
    /// Section name (used for identification).
    name: String,
    /// Section content.
    content: String,
    /// Whether this section is enabled.
    enabled: bool,
}

impl CortexSection {
    fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
            enabled: true,
        }
    }
}

impl CortexPromptBuilder {
    /// Create a new builder with all default sections enabled.
    ///
    /// All nine sections of the Cortex prompt are included by default:
    /// - HEADER: Identity and autonomy level
    /// - PRIME DIRECTIVES: Core execution rules
    /// - COGNITIVE ARCHITECTURE: Five-phase task flow
    /// - FAILURE PROTOCOL: Error handling tiers
    /// - CODE DISCIPLINE: Style, security, operations
    /// - QUALITY CHECKPOINTS: Verification checklist
    /// - TOOLKIT: Available tools
    /// - RESPONSE PATTERNS: Common request handling
    /// - ANTI-PATTERNS: What to avoid
    /// - OUTPUT FORMAT: Completion reporting
    #[must_use]
    pub fn new() -> Self {
        Self {
            sections: vec![
                CortexSection::new("HEADER", SECTION_HEADER),
                CortexSection::new("PRIME DIRECTIVES", SECTION_PRIME_DIRECTIVES),
                CortexSection::new("COGNITIVE ARCHITECTURE", SECTION_COGNITIVE_ARCHITECTURE),
                CortexSection::new("FAILURE PROTOCOL", SECTION_FAILURE_PROTOCOL),
                CortexSection::new("CODE DISCIPLINE", SECTION_CODE_DISCIPLINE),
                CortexSection::new("QUALITY CHECKPOINTS", SECTION_QUALITY_CHECKPOINTS),
                CortexSection::new("TOOLKIT", SECTION_TOOLKIT),
                CortexSection::new("RESPONSE PATTERNS", SECTION_RESPONSE_PATTERNS),
                CortexSection::new("ANTI-PATTERNS", SECTION_ANTI_PATTERNS),
                CortexSection::new("OUTPUT FORMAT", SECTION_OUTPUT_FORMAT),
            ],
            custom_tools: Vec::new(),
            use_custom_toolkit_only: false,
        }
    }

    /// Disable a section by name.
    ///
    /// Section names are case-insensitive. Valid names:
    /// - "HEADER"
    /// - "PRIME DIRECTIVES"
    /// - "COGNITIVE ARCHITECTURE"
    /// - "FAILURE PROTOCOL"
    /// - "CODE DISCIPLINE"
    /// - "QUALITY CHECKPOINTS"
    /// - "TOOLKIT"
    /// - "RESPONSE PATTERNS"
    /// - "ANTI-PATTERNS"
    /// - "OUTPUT FORMAT"
    ///
    /// # Example
    ///
    /// ```rust
    /// use cortex_prompt_harness::prompts::core::CortexPromptBuilder;
    ///
    /// let prompt = CortexPromptBuilder::new()
    ///     .without_section("ANTI-PATTERNS")
    ///     .without_section("RESPONSE PATTERNS")
    ///     .build();
    /// ```
    #[must_use]
    pub fn without_section(mut self, section_name: &str) -> Self {
        let name_upper = section_name.to_uppercase();
        for section in &mut self.sections {
            if section.name.to_uppercase() == name_upper {
                section.enabled = false;
                break;
            }
        }
        self
    }

    /// Enable a previously disabled section by name.
    ///
    /// Section names are case-insensitive.
    #[must_use]
    pub fn with_section(mut self, section_name: &str) -> Self {
        let name_upper = section_name.to_uppercase();
        for section in &mut self.sections {
            if section.name.to_uppercase() == name_upper {
                section.enabled = true;
                break;
            }
        }
        self
    }

    /// Add a custom tool to the toolkit section.
    ///
    /// The tool will be appended to the default toolkit (unless `with_custom_toolkit`
    /// has been called).
    ///
    /// # Example
    ///
    /// ```rust
    /// use cortex_prompt_harness::prompts::core::CortexPromptBuilder;
    ///
    /// let prompt = CortexPromptBuilder::new()
    ///     .add_tool("Analyze", "Analyze code for issues")
    ///     .add_tool("Refactor", "Refactor code automatically")
    ///     .build();
    /// ```
    #[must_use]
    pub fn add_tool(mut self, name: &str, description: &str) -> Self {
        self.custom_tools
            .push((name.to_string(), description.to_string()));
        self
    }

    /// Add multiple tools at once.
    ///
    /// # Example
    ///
    /// ```rust
    /// use cortex_prompt_harness::prompts::core::CortexPromptBuilder;
    ///
    /// let prompt = CortexPromptBuilder::new()
    ///     .with_tools(&[
    ///         ("Analyze", "Analyze code"),
    ///         ("Refactor", "Refactor code"),
    ///     ])
    ///     .build();
    /// ```
    #[must_use]
    pub fn with_tools(mut self, tools: &[(&str, &str)]) -> Self {
        for (name, description) in tools {
            self.custom_tools
                .push(((*name).to_string(), (*description).to_string()));
        }
        self
    }

    /// Replace the toolkit section entirely with custom tools.
    ///
    /// This will remove all default tools and only include the specified ones.
    ///
    /// # Example
    ///
    /// ```rust
    /// use cortex_prompt_harness::prompts::core::CortexPromptBuilder;
    ///
    /// let prompt = CortexPromptBuilder::new()
    ///     .with_custom_toolkit(&[
    ///         ("Read", "Read file contents"),
    ///         ("Write", "Write to files"),
    ///     ])
    ///     .build();
    /// ```
    #[must_use]
    pub fn with_custom_toolkit(mut self, tools: &[(&str, &str)]) -> Self {
        self.use_custom_toolkit_only = true;
        self.custom_tools.clear();
        for (name, description) in tools {
            self.custom_tools
                .push(((*name).to_string(), (*description).to_string()));
        }
        self
    }

    /// Add a custom section with the given name and content.
    ///
    /// The section will be added at the end of the prompt.
    ///
    /// # Example
    ///
    /// ```rust
    /// use cortex_prompt_harness::prompts::core::CortexPromptBuilder;
    ///
    /// let prompt = CortexPromptBuilder::new()
    ///     .add_custom_section("SPECIAL RULES", "## SPECIAL RULES\n\nFollow these special rules...")
    ///     .build();
    /// ```
    #[must_use]
    pub fn add_custom_section(mut self, name: &str, content: &str) -> Self {
        self.sections
            .push(CortexSection::new(name.to_string(), content.to_string()));
        self
    }

    /// Check if a section is enabled.
    #[must_use]
    pub fn is_section_enabled(&self, section_name: &str) -> bool {
        let name_upper = section_name.to_uppercase();
        self.sections
            .iter()
            .any(|s| s.name.to_uppercase() == name_upper && s.enabled)
    }

    /// Get the list of enabled section names.
    #[must_use]
    pub fn enabled_sections(&self) -> Vec<&str> {
        self.sections
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Build the toolkit section with optional custom tools.
    fn build_toolkit_section(&self) -> String {
        if self.use_custom_toolkit_only {
            // Build a custom toolkit from scratch
            let mut content = String::from("## 06 // TOOLKIT\n\n");
            content.push_str("| Tool | Function |\n");
            content.push_str("|------|----------|\n");
            for (name, description) in &self.custom_tools {
                content.push_str(&format!("| `{}` | {} |\n", name, description));
            }
            content
        } else if self.custom_tools.is_empty() {
            // Use the default toolkit as-is
            SECTION_TOOLKIT.to_string()
        } else {
            // Append custom tools to the default toolkit
            let mut content = SECTION_TOOLKIT.to_string();
            content.push_str("\n\n### Custom\n");
            content.push_str("| Tool | Function |\n");
            content.push_str("|------|----------|\n");
            for (name, description) in &self.custom_tools {
                content.push_str(&format!("| `{}` | {} |\n", name, description));
            }
            content
        }
    }

    /// Build the final prompt string.
    ///
    /// Returns the complete Cortex system prompt with all enabled sections
    /// and any custom tools or sections that have been added.
    #[must_use]
    pub fn build(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        for section in &self.sections {
            if !section.enabled {
                continue;
            }

            if section.name == "TOOLKIT" {
                parts.push(self.build_toolkit_section());
            } else {
                parts.push(section.content.clone());
            }
        }

        parts.join("\n\n---\n\n")
    }

    /// Build the prompt and return an estimated token count.
    ///
    /// Uses a simple approximation of ~4 characters per token.
    #[must_use]
    pub fn build_with_token_estimate(&self) -> (String, u32) {
        let prompt = self.build();
        let tokens = (prompt.len() as f64 / 4.0).ceil() as u32;
        (prompt, tokens)
    }
}

impl Default for CortexPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Original CORTEX_MAIN_PROMPT (kept for backward compatibility)
// =============================================================================

/// The main Cortex system prompt - defines the autonomous coding agent.
///
/// This prompt establishes:
/// - Prime directives for autonomous operation
/// - Cognitive architecture (RECON → DESIGN → BUILD → VERIFY → CLOSE)
/// - Failure protocols and recovery strategies
/// - Code discipline and security rules
/// - Quality checkpoints
///
/// # Usage
///
/// This prompt is loaded via `include_str!` from `cortex_prompt.txt` in the
/// main cortex-engine, but the canonical version is defined here.
pub const CORTEX_MAIN_PROMPT: &str = r#"# CORTEX

You are **Cortex**, an autonomous software engineering intelligence.

You operate without human intervention. No confirmations. No prompts. No hand-holding.
You receive a task. You execute it. You verify it. You report completion.

```
AUTONOMY LEVEL: FULL
INTERACTION MODE: ASYNC
VERIFICATION: MANDATORY
```

---

## 01 // PRIME DIRECTIVES

```yaml
execute:
  - Complete the task entirely before stopping
  - Verify everything works before reporting done
  - Match exactly what was requested, nothing extra

forbidden:
  - Asking for clarification (research until you understand)
  - Creating docs/READMEs unless explicitly requested
  - Leaving broken code behind
  - Giving up without exhausting all options
```

When asked about Cortex capabilities → `Fetch` the documentation first.

---

## 02 // COGNITIVE ARCHITECTURE

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

### RECON
> Understand before touching anything.

What to do:
- Scan project structure, find README or docs
- Identify patterns, conventions, dependencies
- Map what exists before planning what to add

Tools: `Read` `Tree` `Search` `Find` `Fetch` `WebQuery`

### DESIGN  
> Plan the attack. Break it down.

What to do:
- Decompose into atomic steps
- Identify risks and dependencies
- Decide what to delegate to sub-agents

Tools: `Plan` `Propose` `Delegate`

### BUILD
> Execute with precision. One change at a time.

What to do:
- Implement step by step
- Respect existing code style religiously
- Verify each change before the next

Tools: `Write` `Patch` `Shell` `Delegate`

### VERIFY
> Trust nothing. Test everything.

What to do:
- Run linters, type checkers, tests
- Confirm requirements are met
- Check for regressions

Tools: `Shell` `Read` `Search`

### CLOSE
> Wrap it up clean.

What to do:
- Summarize in 1-4 sentences
- Mark all tasks complete in `Plan`
- Note any caveats or follow-ups

Tools: `Plan`

---

## 03 // FAILURE PROTOCOL

When something breaks, escalate systematically:

```
TIER 1: RETRY
├── Read the error carefully
├── Check paths, typos, syntax
├── Try slight variations
└── Max 3 attempts → escalate

TIER 2: PIVOT  
├── Undo what broke things
├── Research alternatives
├── Try different approach
└── Consult docs via Fetch/WebQuery

TIER 3: DECOMPOSE
├── Break into smaller pieces
├── Isolate the failing part
├── Solve pieces independently
└── Delegate if needed

TIER 4: GRACEFUL EXIT
├── Document what was tried
├── Explain the blocker
├── Suggest workarounds
├── Complete what's possible
└── Leave code in working state
```

**Hard rule**: Never leave the codebase broken. Rollback if needed.

---

## 04 // CODE DISCIPLINE

### Style
```
READ first, CODE second.
MATCH the existing patterns.
VERIFY libraries exist before importing.
```

### Security
```
NEVER expose: keys, secrets, tokens, passwords
NEVER log sensitive data, even in debug
ALWAYS sanitize inputs
ALWAYS use secure defaults
```

### Operations
```
PREFER Patch over Write for existing files
ALWAYS Read before Patch
THINK rollback before every change
```

---

## 05 // QUALITY CHECKPOINTS

Run these checks at each phase:

```
BEFORE ACTION
├── Requirement understood?
├── Relevant files read?
├── Side effects mapped?
├── Right tool selected?
└── Following existing patterns?

AFTER ACTION
├── Change applied correctly?
├── No syntax errors?
├── Functionality preserved?
└── Style consistent?

BEFORE COMPLETION
├── All requirements met?
├── Tests passing?
├── No errors in system messages?
├── Summary ready?
└── Plan updated?
```

Find and run the project's verification commands:
- Linter (eslint, pylint, etc.)
- Type checker (tsc, mypy, etc.)
- Tests (jest, pytest, etc.)

---

## 06 // TOOLKIT

### Perception
| Tool | Function |
|------|----------|
| `Read` | Read file contents |
| `Tree` | Show directory structure |
| `Search` | Regex search in files |
| `Find` | Glob pattern file discovery |
| `Fetch` | Get URL content |
| `WebQuery` | Search the web |

### Action  
| Tool | Function |
|------|----------|
| `Write` | Create new files |
| `Patch` | Edit existing files |
| `Shell` | Run commands |

### Cognition
| Tool | Function |
|------|----------|
| `Plan` | Track task progress |
| `Propose` | Present plans for approval |

### Collaboration
| Tool | Function |
|------|----------|
| `Delegate` | Send task to sub-agent |
| `UseSkill` | Invoke specialized skill |
| `CreateAgent` | Define new agent |

---

## 07 // RESPONSE PATTERNS

```
"read X"           → Read      → Brief summary
"list files"       → Tree      → Structure + context  
"search for X"     → Search    → Concise findings
"find files like"  → Find      → Path list
"create file"      → Write     → Confirm done
"edit/change"      → Patch     → Confirm change
"run command"      → Shell     → Relevant output
"look up online"   → WebQuery  → Key results
"handle subtask"   → Delegate  → Agent result
```

---

## 08 // ANTI-PATTERNS

```diff
- Adding features not requested
- Doing "related" work without being asked
- Taking shortcuts or hacks
- Jumping to code before understanding
- Surrendering when hitting obstacles
- Assuming dependencies exist
- Ignoring project conventions
```

---

## 09 // OUTPUT FORMAT

When done:
```
Brief summary of what was accomplished (1-4 sentences).
Any caveats or follow-up items if relevant.
```

No excessive detail. No self-congratulation. Just facts.
"#;

/// System prompt template for the TUI agent.
///
/// This template uses placeholders for dynamic values:
/// - `{cwd}` - Current working directory
/// - `{date}` - Current date
/// - `{platform}` - Operating system
/// - `{is_git}` - Whether current directory is a git repo
///
/// # Usage
///
/// ```rust
/// use cortex_prompt_harness::prompts::core::TUI_SYSTEM_PROMPT_TEMPLATE;
///
/// let prompt = TUI_SYSTEM_PROMPT_TEMPLATE
///     .replace("{cwd}", "/my/project")
///     .replace("{date}", "Mon Jan 15 2024")
///     .replace("{platform}", "linux")
///     .replace("{is_git}", "true");
/// ```
pub const TUI_SYSTEM_PROMPT_TEMPLATE: &str = r#"You are Cortex, an expert AI coding assistant.

# Environment
- Working directory: {cwd}
- Date: {date}
- Platform: {platform}
- Git repository: {is_git}

# Tone and Style
- Be concise but thorough
- Only use emojis if explicitly requested
- Output text to communicate; use tools to complete tasks
- NEVER create files unless absolutely necessary
- Prefer editing existing files over creating new ones

# Tool Usage Policy
- Read files before editing to understand context
- Make targeted edits rather than full rewrites
- Use Glob for file pattern matching, Grep for content search
- Use Task tool for delegating complex sub-tasks
- For shell commands, explain what they do before executing
- Call multiple tools in parallel when operations are independent

# Todo List (IMPORTANT)
For any non-trivial task that requires multiple steps:
- Use the TodoWrite tool immediately to create a todo list tracking your progress
- Update the todo list as you complete each step (mark items as in_progress or completed)
- This provides real-time visibility to the user on what you're working on
- Keep only ONE item as in_progress at a time

# Guidelines
- Always verify paths exist before operations
- Handle errors gracefully and suggest alternatives
- Ask clarifying questions when requirements are ambiguous
- Prioritize technical accuracy over validating assumptions

# Code Quality
- Follow existing code style and conventions
- Add comments for complex logic
- Write self-documenting code with clear naming
- Consider edge cases and error handling
"#;

/// Build the TUI system prompt with current environment values.
pub fn build_tui_system_prompt() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let date = chrono::Local::now().format("%a %b %d %Y").to_string();
    let platform = std::env::consts::OS;
    let is_git = std::path::Path::new(".git").exists();

    TUI_SYSTEM_PROMPT_TEMPLATE
        .replace("{cwd}", &cwd)
        .replace("{date}", &date)
        .replace("{platform}", platform)
        .replace("{is_git}", &is_git.to_string())
}

/// Context strings for capability injection into system prompts.
pub mod capabilities {
    /// Code execution capability context.
    pub const CODE_EXECUTION: &str = r#"## Code Execution
You have access to execute shell commands and code. Use this capability responsibly:
- Always explain what commands will do before executing
- Prefer non-destructive operations
- Ask for confirmation before making significant changes
- Handle errors gracefully"#;

    /// File operations capability context.
    pub const FILE_OPERATIONS: &str = r#"## File Operations
You can read, write, and modify files. Guidelines:
- Read files to understand context before making changes
- Make targeted edits rather than rewriting entire files
- Create backups when making significant changes
- Respect file permissions and ownership"#;

    /// Web search capability context.
    pub const WEB_SEARCH: &str = r#"## Web Search
You can search the web for information. Guidelines:
- Use specific, targeted searches
- Cite sources when providing information
- Verify information from multiple sources when possible
- Be clear about the recency of information"#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cortex_main_prompt_contains_key_sections() {
        assert!(CORTEX_MAIN_PROMPT.contains("PRIME DIRECTIVES"));
        assert!(CORTEX_MAIN_PROMPT.contains("COGNITIVE ARCHITECTURE"));
        assert!(CORTEX_MAIN_PROMPT.contains("FAILURE PROTOCOL"));
        assert!(CORTEX_MAIN_PROMPT.contains("CODE DISCIPLINE"));
        assert!(CORTEX_MAIN_PROMPT.contains("QUALITY CHECKPOINTS"));
    }

    #[test]
    fn test_tui_template_has_placeholders() {
        assert!(TUI_SYSTEM_PROMPT_TEMPLATE.contains("{cwd}"));
        assert!(TUI_SYSTEM_PROMPT_TEMPLATE.contains("{date}"));
        assert!(TUI_SYSTEM_PROMPT_TEMPLATE.contains("{platform}"));
        assert!(TUI_SYSTEM_PROMPT_TEMPLATE.contains("{is_git}"));
    }

    #[test]
    fn test_build_tui_system_prompt() {
        let prompt = build_tui_system_prompt();
        assert!(prompt.contains("Cortex"));
        assert!(prompt.contains("Working directory:"));
        // Placeholders should be replaced
        assert!(!prompt.contains("{cwd}"));
        assert!(!prompt.contains("{date}"));
    }

    // =========================================================================
    // CortexPromptBuilder Tests
    // =========================================================================

    #[test]
    fn test_section_constants_contain_expected_content() {
        assert!(SECTION_HEADER.contains("# CORTEX"));
        assert!(SECTION_HEADER.contains("autonomous software engineering intelligence"));

        assert!(SECTION_PRIME_DIRECTIVES.contains("01 // PRIME DIRECTIVES"));
        assert!(SECTION_PRIME_DIRECTIVES.contains("execute:"));
        assert!(SECTION_PRIME_DIRECTIVES.contains("forbidden:"));

        assert!(SECTION_COGNITIVE_ARCHITECTURE.contains("02 // COGNITIVE ARCHITECTURE"));
        assert!(SECTION_COGNITIVE_ARCHITECTURE.contains("RECON"));
        assert!(SECTION_COGNITIVE_ARCHITECTURE.contains("DESIGN"));
        assert!(SECTION_COGNITIVE_ARCHITECTURE.contains("BUILD"));
        assert!(SECTION_COGNITIVE_ARCHITECTURE.contains("VERIFY"));
        assert!(SECTION_COGNITIVE_ARCHITECTURE.contains("CLOSE"));

        assert!(SECTION_FAILURE_PROTOCOL.contains("03 // FAILURE PROTOCOL"));
        assert!(SECTION_FAILURE_PROTOCOL.contains("TIER 1: RETRY"));
        assert!(SECTION_FAILURE_PROTOCOL.contains("TIER 4: GRACEFUL EXIT"));

        assert!(SECTION_CODE_DISCIPLINE.contains("04 // CODE DISCIPLINE"));
        assert!(SECTION_CODE_DISCIPLINE.contains("Security"));
        assert!(SECTION_CODE_DISCIPLINE.contains("NEVER expose"));

        assert!(SECTION_QUALITY_CHECKPOINTS.contains("05 // QUALITY CHECKPOINTS"));
        assert!(SECTION_QUALITY_CHECKPOINTS.contains("BEFORE ACTION"));
        assert!(SECTION_QUALITY_CHECKPOINTS.contains("AFTER ACTION"));

        assert!(SECTION_TOOLKIT.contains("06 // TOOLKIT"));
        assert!(SECTION_TOOLKIT.contains("Perception"));
        assert!(SECTION_TOOLKIT.contains("Action"));
        assert!(SECTION_TOOLKIT.contains("`Read`"));

        assert!(SECTION_RESPONSE_PATTERNS.contains("07 // RESPONSE PATTERNS"));
        assert!(SECTION_RESPONSE_PATTERNS.contains("read X"));

        assert!(SECTION_ANTI_PATTERNS.contains("08 // ANTI-PATTERNS"));
        assert!(SECTION_ANTI_PATTERNS.contains("Adding features not requested"));

        assert!(SECTION_OUTPUT_FORMAT.contains("09 // OUTPUT FORMAT"));
        assert!(SECTION_OUTPUT_FORMAT.contains("Brief summary"));
    }

    #[test]
    fn test_builder_default_creates_full_prompt() {
        let builder = CortexPromptBuilder::new();
        let prompt = builder.build();

        // Should contain all default sections
        assert!(prompt.contains("# CORTEX"));
        assert!(prompt.contains("PRIME DIRECTIVES"));
        assert!(prompt.contains("COGNITIVE ARCHITECTURE"));
        assert!(prompt.contains("FAILURE PROTOCOL"));
        assert!(prompt.contains("CODE DISCIPLINE"));
        assert!(prompt.contains("QUALITY CHECKPOINTS"));
        assert!(prompt.contains("TOOLKIT"));
        assert!(prompt.contains("RESPONSE PATTERNS"));
        assert!(prompt.contains("ANTI-PATTERNS"));
        assert!(prompt.contains("OUTPUT FORMAT"));
    }

    #[test]
    fn test_builder_without_section() {
        let prompt = CortexPromptBuilder::new()
            .without_section("ANTI-PATTERNS")
            .build();

        assert!(prompt.contains("PRIME DIRECTIVES"));
        assert!(!prompt.contains("08 // ANTI-PATTERNS"));
    }

    #[test]
    fn test_builder_without_multiple_sections() {
        let prompt = CortexPromptBuilder::new()
            .without_section("ANTI-PATTERNS")
            .without_section("RESPONSE PATTERNS")
            .without_section("OUTPUT FORMAT")
            .build();

        assert!(prompt.contains("PRIME DIRECTIVES"));
        assert!(prompt.contains("TOOLKIT"));
        assert!(!prompt.contains("08 // ANTI-PATTERNS"));
        assert!(!prompt.contains("07 // RESPONSE PATTERNS"));
        assert!(!prompt.contains("09 // OUTPUT FORMAT"));
    }

    #[test]
    fn test_builder_section_names_case_insensitive() {
        let prompt1 = CortexPromptBuilder::new()
            .without_section("anti-patterns")
            .build();

        let prompt2 = CortexPromptBuilder::new()
            .without_section("ANTI-PATTERNS")
            .build();

        let prompt3 = CortexPromptBuilder::new()
            .without_section("Anti-Patterns")
            .build();

        // All should produce the same result
        assert!(!prompt1.contains("08 // ANTI-PATTERNS"));
        assert!(!prompt2.contains("08 // ANTI-PATTERNS"));
        assert!(!prompt3.contains("08 // ANTI-PATTERNS"));
    }

    #[test]
    fn test_builder_with_section_re_enables() {
        let prompt = CortexPromptBuilder::new()
            .without_section("ANTI-PATTERNS")
            .with_section("ANTI-PATTERNS")
            .build();

        assert!(prompt.contains("08 // ANTI-PATTERNS"));
    }

    #[test]
    fn test_builder_add_tool() {
        let prompt = CortexPromptBuilder::new()
            .add_tool("MyTool", "Does custom things")
            .build();

        assert!(prompt.contains("### Custom"));
        assert!(prompt.contains("`MyTool`"));
        assert!(prompt.contains("Does custom things"));
        // Should still have default tools
        assert!(prompt.contains("`Read`"));
        assert!(prompt.contains("`Write`"));
    }

    #[test]
    fn test_builder_with_tools() {
        let prompt = CortexPromptBuilder::new()
            .with_tools(&[
                ("Tool1", "Description 1"),
                ("Tool2", "Description 2"),
            ])
            .build();

        assert!(prompt.contains("`Tool1`"));
        assert!(prompt.contains("Description 1"));
        assert!(prompt.contains("`Tool2`"));
        assert!(prompt.contains("Description 2"));
        // Should still have default tools
        assert!(prompt.contains("`Read`"));
    }

    #[test]
    fn test_builder_with_custom_toolkit() {
        let prompt = CortexPromptBuilder::new()
            .with_custom_toolkit(&[
                ("OnlyTool1", "Only description 1"),
                ("OnlyTool2", "Only description 2"),
            ])
            .build();

        assert!(prompt.contains("`OnlyTool1`"));
        assert!(prompt.contains("Only description 1"));
        assert!(prompt.contains("`OnlyTool2`"));
        // Should NOT have default toolkit tools in the TOOLKIT section
        // Note: Some tool names appear in other sections (e.g., COGNITIVE ARCHITECTURE)
        // so we check the toolkit section structure instead
        assert!(!prompt.contains("### Perception"));
        assert!(!prompt.contains("### Collaboration"));
        assert!(!prompt.contains("| `Delegate` |"));
    }

    #[test]
    fn test_builder_custom_toolkit_replaces_add_tool() {
        let prompt = CortexPromptBuilder::new()
            .add_tool("FirstTool", "Should be replaced")
            .with_custom_toolkit(&[
                ("FinalTool", "Final description"),
            ])
            .build();

        assert!(!prompt.contains("FirstTool"));
        assert!(!prompt.contains("Should be replaced"));
        assert!(prompt.contains("`FinalTool`"));
        assert!(prompt.contains("Final description"));
    }

    #[test]
    fn test_builder_add_custom_section() {
        let prompt = CortexPromptBuilder::new()
            .add_custom_section("SPECIAL RULES", "## SPECIAL RULES\n\nFollow these special rules...")
            .build();

        assert!(prompt.contains("## SPECIAL RULES"));
        assert!(prompt.contains("Follow these special rules"));
    }

    #[test]
    fn test_builder_is_section_enabled() {
        let builder = CortexPromptBuilder::new()
            .without_section("ANTI-PATTERNS");

        assert!(builder.is_section_enabled("PRIME DIRECTIVES"));
        assert!(builder.is_section_enabled("TOOLKIT"));
        assert!(!builder.is_section_enabled("ANTI-PATTERNS"));
        assert!(!builder.is_section_enabled("anti-patterns")); // case insensitive
    }

    #[test]
    fn test_builder_enabled_sections() {
        let builder = CortexPromptBuilder::new()
            .without_section("ANTI-PATTERNS")
            .without_section("RESPONSE PATTERNS");

        let enabled = builder.enabled_sections();

        assert!(enabled.contains(&"HEADER"));
        assert!(enabled.contains(&"PRIME DIRECTIVES"));
        assert!(enabled.contains(&"TOOLKIT"));
        assert!(!enabled.contains(&"ANTI-PATTERNS"));
        assert!(!enabled.contains(&"RESPONSE PATTERNS"));
    }

    #[test]
    fn test_builder_build_with_token_estimate() {
        let (prompt, tokens) = CortexPromptBuilder::new().build_with_token_estimate();

        assert!(!prompt.is_empty());
        assert!(tokens > 0);
        // Rough check: token estimate should be approximately len/4
        let expected_approx = (prompt.len() as f64 / 4.0).ceil() as u32;
        assert_eq!(tokens, expected_approx);
    }

    #[test]
    fn test_builder_default_trait() {
        let builder1 = CortexPromptBuilder::new();
        let builder2 = CortexPromptBuilder::default();

        let prompt1 = builder1.build();
        let prompt2 = builder2.build();

        assert_eq!(prompt1, prompt2);
    }

    #[test]
    fn test_builder_sections_separated_by_divider() {
        let prompt = CortexPromptBuilder::new().build();

        // Sections should be separated by "---"
        assert!(prompt.contains("\n\n---\n\n"));
    }

    #[test]
    fn test_builder_only_header() {
        let prompt = CortexPromptBuilder::new()
            .without_section("PRIME DIRECTIVES")
            .without_section("COGNITIVE ARCHITECTURE")
            .without_section("FAILURE PROTOCOL")
            .without_section("CODE DISCIPLINE")
            .without_section("QUALITY CHECKPOINTS")
            .without_section("TOOLKIT")
            .without_section("RESPONSE PATTERNS")
            .without_section("ANTI-PATTERNS")
            .without_section("OUTPUT FORMAT")
            .build();

        assert!(prompt.contains("# CORTEX"));
        assert!(!prompt.contains("PRIME DIRECTIVES"));
        assert!(!prompt.contains("TOOLKIT"));
    }

    #[test]
    fn test_section_names_constant() {
        assert_eq!(SECTION_NAMES.len(), 10);
        assert!(SECTION_NAMES.contains(&"HEADER"));
        assert!(SECTION_NAMES.contains(&"PRIME DIRECTIVES"));
        assert!(SECTION_NAMES.contains(&"COGNITIVE ARCHITECTURE"));
        assert!(SECTION_NAMES.contains(&"FAILURE PROTOCOL"));
        assert!(SECTION_NAMES.contains(&"CODE DISCIPLINE"));
        assert!(SECTION_NAMES.contains(&"QUALITY CHECKPOINTS"));
        assert!(SECTION_NAMES.contains(&"TOOLKIT"));
        assert!(SECTION_NAMES.contains(&"RESPONSE PATTERNS"));
        assert!(SECTION_NAMES.contains(&"ANTI-PATTERNS"));
        assert!(SECTION_NAMES.contains(&"OUTPUT FORMAT"));
    }

    #[test]
    fn test_builder_fluent_chaining() {
        // Test that fluent API works correctly with method chaining
        let prompt = CortexPromptBuilder::new()
            .without_section("ANTI-PATTERNS")
            .without_section("RESPONSE PATTERNS")
            .add_tool("CustomA", "Desc A")
            .add_tool("CustomB", "Desc B")
            .add_custom_section("EXTRA", "## EXTRA\n\nExtra content")
            .build();

        assert!(!prompt.contains("08 // ANTI-PATTERNS"));
        assert!(!prompt.contains("07 // RESPONSE PATTERNS"));
        assert!(prompt.contains("`CustomA`"));
        assert!(prompt.contains("`CustomB`"));
        assert!(prompt.contains("## EXTRA"));
    }

    #[test]
    fn test_builder_without_toolkit_section() {
        let prompt = CortexPromptBuilder::new()
            .without_section("TOOLKIT")
            .build();

        assert!(!prompt.contains("06 // TOOLKIT"));
        // Check that toolkit-specific structure is missing
        assert!(!prompt.contains("### Perception"));
        assert!(!prompt.contains("### Collaboration"));
        assert!(!prompt.contains("| `Delegate` |"));
    }

    #[test]
    fn test_builder_clone() {
        let builder = CortexPromptBuilder::new()
            .without_section("ANTI-PATTERNS")
            .add_tool("MyTool", "My description");

        let cloned = builder.clone();

        assert_eq!(builder.build(), cloned.build());
    }
}
