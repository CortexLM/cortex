# Changelog

All notable changes to Cortex CLI will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## 0.0.5

### Added
- Added interactive /agents command for agent management with full TUI support
- Added interactive /mcp panel for centralized MCP server management
- Added multi-step wizard for adding MCP servers with guided configuration
- Added --debug flag to write all trace logs to ./debug.txt for troubleshooting
- Added async loading panels for /billing and /account commands
- Added artifact system for large tool outputs to prevent payload errors
- Added real-time todos display for subagents during task execution
- Added mouse hover and click support for interactive mode
- Added scrollbar click-drag scrolling and hover interaction in TUI
- Added scrollbar support to dropdown menus
- Added TUI capture and snapshot testing framework for debugging
- Added TUI backtracking, file mentions, and external editor support (Ctrl+G)
- Added custom slash commands and agents system
- Added Plan Mode & Spec Mode system for structured task execution
- Added session favorites, tags, sharing, and search functionality
- Added multi-agent collaboration system with orchestrator support
- Added mandatory planning phase and final summary for orchestrator subagents
- Added hooks system with async events and LLM prompts
- Added advanced network proxy SSRF protection and sandbox modes
- Added session filtering and date filtering options
- Added styled output with theme-aware colors
- Added colorful styled help output with categorized options
- Added support for agent.md format (.agents/, .agent/) for agents and skills
- Added fault tolerance and robust defaults for corrupted config files

### Fixed
- Fixed agent creation to be fully automated when using --generate flag
- Fixed subagent max iterations increased from 10 to 500 for complex tasks
- Fixed subagent silent errors with improved logging
- Fixed subagent crash error messages now properly propagated
- Fixed false 'task crashed' errors due to race condition
- Fixed subagent fallback response when no text output is produced
- Fixed subagent display format with proper timer tracking
- Fixed 'Starting...' status changed to 'Processing request...' for clarity
- Fixed HTTP 413 payload errors by displaying todos separately
- Fixed frame capture rate limited to max 1 frame per second
- Fixed paste functionality in MCP server configuration modal
- Fixed MCP panel navigation to return after adding server
- Fixed MCP servers enabled by default after creation
- Fixed content ordering and streaming cursor display
- Fixed tool handler names aligned with registry expectations
- Fixed false session expiration during the last hour
- Fixed auth token reload into provider_manager after login
- Fixed expired token removal to prevent 'Already Logged In' dialog
- Fixed auto-redirect to login on session expiration
- Fixed scrollbar position to reflect viewport, not selection
- Fixed missing slash commands registration (init, commands, agents, share)
- Fixed full Unicode symbols preservation in TUI frame capture
- Fixed Windows Credential Manager 2560-byte BLOB size limit
- Fixed logout to clear all storage locations including fallback
- Fixed Windows keyring issues with fallback storage
- Fixed stale credentials cleared before saving new auth tokens
- Fixed context window configuration options exposed in default config
- Fixed model alias resolution in ACP server command
- Fixed dark green colors replaced with standard theme colors
- Fixed bold text color changed from blue to green in markdown
- Fixed AccessibilityHelp dialog no longer interferes with typing '?'
- Fixed terminal stream closure handling and memory leak prevention
- Fixed terminal duration metadata handling with defensive utilities

### Changed
- Changed MCP management from modal popup to inline card UI
- Changed CLI output to remove emojis for cleaner terminal display
- Changed error messages and logs to remove emojis
- Changed agents TUI with improved navigation
- Changed subagent display to Factory-style todos format
- Changed tool handlers to remove deprecated handlers and fix naming
- Changed 'droid' renamed to 'agent' throughout codebase
- Changed settings to persist in ~/.cortex on Linux/macOS
- Changed MCP server configurations to persist to local data directory
