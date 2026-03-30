# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**ClaudeCat** is a **Deep Analysis CLI Tool** for Claude Code — it goes beyond what `/init` provides by using AST-based parsing, cross-file flow tracing, and confidence-scored pattern detection to give Claude Code deep project awareness.

**Positioning**: `/init` tells Claude **what** your project is. ClaudeCat tells Claude **how** your project actually works.

**Current Status**: Pivoting from MCP server to **CLI tool + Claude Code skill**. Core analysis engine is production-ready; new delivery interface is in planning.

## Project Structure

```
/home/yanggf/a/claudecat/
├── src/
│   ├── core/                 # Core analysis engine (reused by all interfaces)
│   │   ├── project-detector.ts      # AST-based pattern detection
│   │   ├── claude-md-maintainer.ts  # CLAUDE.md maintenance
│   │   ├── context-watcher.ts       # File change monitoring
│   │   └── proactive-context-engine.ts # Main engine orchestrator
│   ├── types/                # TypeScript type definitions
│   ├── cli.ts                # CLI entry point (planned)
│   ├── server.ts             # MCP server (legacy)
│   ├── stdio-mcp-server.ts   # Stdio MCP server (legacy)
│   └── stdio-mcp-logger.ts   # Session logging (legacy)
├── skill/                    # Claude Code skill definition (planned)
├── scripts/                  # Installation and testing scripts
├── dist/                     # Compiled JavaScript output
└── docs/                     # Project documentation
    ├── CLAUDECAT-PROPOSAL.md
    ├── CLAUDECAT-ARCHITECTURE.md
    └── CLAUDECAT-GOALS.md
```

## Core Concept

ClaudeCat is a **deep analysis layer** that complements Claude Code's built-in `/init` command:

| Capability | `/init` | ClaudeCat |
|---|---|---|
| Project structure & config scanning | Yes | Inherited |
| AST-based pattern detection | No | Yes |
| Cross-file execution flow tracing | No | Yes |
| Confidence-scored patterns with evidence | No | Yes |
| Dependency graph & symbol resolution | No | Yes |
| Architectural pattern detection (MVC, DI, middleware) | Shallow | Deep |
| Ongoing re-analysis on code changes | No | Yes |

### Design Principles
1. **Deep over Broad** - AST-based analysis of HOW your project implements features, not just what frameworks exist
2. **Evidence-Based** - Every detected pattern includes confidence scores and source file citations
3. **Complementary** - Enhances `/init` output rather than replacing it; appends implementation-level insights
4. **On-Demand** - CLI tool and Claude Code skill, invoked when deeper analysis is needed

## Architecture: Analysis Pipeline

The core analysis flows through three layers: **Detection → Maintenance → Delivery**

### 1. Single-File Detection (`src/core/project-detector.ts` — 710 lines)
- Entry: `EnhancedProjectDetector.detectCurrentContext()`
- Scans all JS/TS files via glob, reads package.json
- AST-based pattern matching for auth, API response, and error handling
- Each pattern produces `PatternDetectionSignal` with confidence score and evidence

### 2. Cross-File Analysis (`src/core/` — 5 files, ~2500 lines)
- `ast-parser.ts` → parses imports/exports per file
- `dependency-graph-builder.ts` → builds full project dependency graph
- `symbol-resolver.ts` → resolves cross-file symbol references
- `execution-flow-tracer.ts` → traces multi-file call chains (MVC, middleware, DI)
- `cross-file-pattern-detector.ts` → detects architectural patterns
- Orchestrated by `enhanced-project-detector.ts` which combines single-file + cross-file results

### 3. Maintenance (`src/core/claude-md-maintainer.ts`)
- Reads existing CLAUDE.md, extracts `<!-- claudecat:auto:begin -->` section
- Generates new section from `ProjectContextInfo`
- Atomic write with rollback on failure; only writes when `hasSignificantChanges()` is true

### 4. Orchestration (`src/core/proactive-context-engine.ts`)
- Wires Detection + Maintenance + file watching (chokidar via `ContextWatcher`)
- Re-triggers detection on file changes
- Emits events: `context-updated`, `context-error`, `startup-complete`

### 5. Delivery (pivoting)
- Current: `src/stdio-mcp-server.ts` — 5 MCP tools over stdio
- Planned: `src/cli.ts` (CLI) + `skill/` (Claude Code skill)

## Key Types

- `ProjectContextInfo` (`src/types/patterns.ts`) — main output: detected patterns, project metadata, confidence scores
- `ImplementationPatterns` — auth, API response, and error handling pattern details
- `PatternDetectionSignal` — individual detection with confidence, evidence file, and pattern type
- `CrossFileAnalysisResult` (`src/types/cross-file-analysis.ts`) — dependency graph, symbols, execution paths

## Technology Stack

- **Language**: TypeScript/JavaScript (Node.js)
- **AST Parsing**: @typescript-eslint/typescript-estree
- **File Discovery**: glob, chokidar
- **Schema Validation**: zod
- **Interfaces**: CLI tool (primary), Claude Code skill (planned), MCP server (legacy)

## Installation & Usage

### CLI Tool (Planned - Primary Interface)

```bash
# Install globally
npm install -g claudecat

# Analyze current project (deep patterns beyond /init)
claudecat scan

# Update CLAUDE.md with deep analysis results
claudecat update

# Show detected patterns and confidence scores
claudecat status
```

### Claude Code Skill (Planned)

```bash
# Inside Claude Code session
/claudecat              # Run deep analysis
/claudecat scan         # Scan only, don't update CLAUDE.md
/claudecat status       # Show current pattern detection status
```

### MCP Server (Legacy - Still Functional)

```bash
npm install && npm run build
./scripts/install.sh
claude
```

### Development Commands

```bash
npm run build                          # Compile TypeScript to dist/
npm run dev                            # Watch mode (tsx watch)
npm run typecheck                      # Type-check without emitting
npm run test                           # Jest tests
npm run test:cross-file                # Cross-file analysis integration test
node scripts/run-accuracy-test.js      # Accuracy validation against test projects
node scripts/test-detection.js         # Quick pattern detection smoke test
./scripts/install.sh                   # Register MCP server with Claude Code
./scripts/uninstall.sh                 # Remove MCP server registration
```

## Core Engine Validation (Phases 1-10 Complete)

The analysis engine has been validated across 10 phases of development. Full phase history is in `ARCHITECTURE-STUDY.md`.

### Key Metrics
- **Accuracy**: 25% → 75% on Express + Passport pattern detection (100% on core patterns)
- **Performance**: 57ms processing (68% faster than 181ms baseline), 404 files/second
- **Cross-File**: 43+ files/sec, 900+ symbols resolved, 16 execution paths traced
- **Reliability**: 100MB log rotation, session isolation, EPIPE loop prevention

## Project Status

**Core Engine**: Production-ready (Phases 1-10 complete). Full results in `ARCHITECTURE-STUDY.md`.

**Current Focus**: CLI + Skill delivery interface (Phases 11-12).

## Roadmap

### Phase 11: CLI Tool (Next)
- CLI entry point (`src/cli.ts`) with `scan`, `update`, `status` commands
- Reuse existing core engine (project-detector, claude-md-maintainer)
- `bin` field in package.json for `claudecat` command
- npm-publishable package

### Phase 12: Cloud Storage (Turso)
- Cross-machine pattern sharing (work + home)
- Turso (libSQL) for low-cost cloud persistence
- CLI reads/writes patterns to cloud DB

### Phase 13: Claude Code Skill
- Skill definition in `skill/` directory following Claude Code skill spec
- `/claudecat` slash command invocable inside Claude Code sessions
- Skill calls the same core engine as the CLI
- Can sync with cloud storage from Phase 12
- Optional `SessionStart` hook for auto-invocation

### Phase 14: Team Sharing
- Shared pattern conventions via cloud sync
- Team-wide consistency enforcement

### Future
- **Additional Languages**: Python, Go, Rust AST support — based on demand

**Last Updated**: 2026-03-30\n\n<!-- claudecat:auto:begin:project-context -->
## Project Context (Auto-Maintained by ClaudeCat)

**Project Type**: Express API  
**Language**: TypeScript  
**Framework**: Express.js  
**Package Manager**: npm

### Implementation Patterns

#### Authentication Implementation (100% - High Confidence)
- **User Property**: `req.auth`
- **Token Storage**: Unknown
- **Error Response**: Unknown
- **Middleware Pattern**: app.use(auth)\n  Evidence: /debug-pattern-matching.js: req.auth usage (70% confidence)

#### API Response Implementation (100% - High Confidence)
- **Success Format**: bare object
- **Error Format**: Unknown
- **Status Codes**: default 200/500
- **Wrapper Pattern**: conditional\n  Evidence: /test-performance.js: bare object response (100% confidence), /test-cross-file-analysis.js: {result: any} format (100% confidence), /final-validation.js: bare object response (100% confidence)

#### Error Handling Implementation (100% - High Confidence)
- **Catch Pattern**: global middleware
- **Error Structure**: Unknown
- **Logging Integration**: integrated
- **Propagation Style**: Unknown\n  Evidence: /test-performance.js: global error handler

### Development Information

**Scripts**:
- dev: `tsx watch src/stdio-mcp-server.ts`
- build: `tsc`
- test: `jest`

**Key Directories**:
  - src/ (source code)
  - src/types/ (TypeScript types)

**Core Dependencies**: @libsql/client, @modelcontextprotocol/sdk, @typescript-eslint/typescript-estree, chokidar, glob, zod

### Critical Guardrails

✅ **ALWAYS use `req.auth`** for authenticated user data\n✅ **Use bare object responses** - No wrapper format detected\n✅ **Follow `global middleware`** error handling pattern

**Last Updated**: 2026-03-30T07:19:26.525Z  
**Detection Quality**: Implementation patterns auto-detected with confidence scoring

*This section is automatically maintained by ClaudeCat. All patterns include confidence scores and evidence citations.*
<!-- claudecat:auto:end:project-context -->\n