# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**ClaudeCat** is a "Proactive Context Engine" - a fully implemented MCP server that solves Claude Code's context accuracy problems through proactive implementation pattern detection and automatic CLAUDE.md maintenance.

**Current Status**: ✅ **IMPLEMENTED** - Fully functional MCP server ready for use

## Project Structure

```
/home/yanggf/a/claudecat/
├── src/                      # TypeScript implementation
│   ├── core/                 # Core engine components
│   │   ├── project-detector.ts      # Pattern detection engine
│   │   ├── claude-md-maintainer.ts  # CLAUDE.md maintenance
│   │   ├── context-watcher.ts       # File monitoring
│   │   └── proactive-context-engine.ts # Main engine
│   ├── types/                # TypeScript type definitions
│   └── server.ts             # MCP server with stdio transport
├── scripts/                  # Installation and testing scripts
├── dist/                     # Compiled JavaScript output
├── CLAUDECAT-PROPOSAL.md     # Original proposal documentation
├── CLAUDECAT-ARCHITECTURE.md # Technical architecture details  
├── CLAUDECAT-GOALS.md        # Project objectives and metrics
├── README.md                 # Installation and usage guide
└── CLAUDE.md                 # This file
```

## Core Concept

The project proposes transforming Claude Code from a "context-lacking assistant" to a "project-aware development partner" through:

1. **Proactive Implementation Pattern Detection** - Automatically detect HOW projects implement auth, API responses, error handling
2. **Startup Project Awareness** - Maintain CLAUDE.md with implementation-specific guidance before queries
3. **Real-time Context Maintenance** - Update project awareness as code patterns evolve

## Key Technical Components (Proposed)

### Implementation Pattern Detection Engine
- **Authentication Patterns**: Detect `req.user` vs `req.context.user`, cookie vs header tokens, error formats
- **API Response Patterns**: Identify `{data: any}` vs `{result: any}` vs bare objects, wrapper patterns
- **Error Handling Patterns**: Discover catch patterns, error structures, propagation styles
- **Confidence Scoring**: Evidence-based detection with 60%+ confidence thresholds

### Proactive CLAUDE.md Maintenance
- **Boot-time Context Generation**: Populate implementation patterns before first interaction
- **Marker-based Updates**: Atomic file operations with rollback (`<!-- cortex:auto:begin -->` markers)
- **Critical Guardrails**: Project-specific constraints (e.g., "NEVER use localStorage for tokens")

### Real-time Monitoring
- **File Change Detection**: Monitor auth, API, and error handling files for pattern changes
- **Debounced Updates**: 500ms settling time for batch changes
- **Pattern Evolution**: Maintain accuracy as codebase evolves

## Proposed Technology Stack

- **Language**: TypeScript/JavaScript (Node.js)
- **Communication**: MCP (Model Context Protocol) with stdio transport
- **File Watching**: chokidar for monitoring changes
- **Protocol**: JSON-RPC 2.0 over stdin/stdout
- **Integration**: Claude Code MCP server registration

## Installation & Usage

```bash
# Install dependencies and build
npm install
npm run build

# Install and register with Claude Code
./scripts/install.sh

# Start Claude Code with MCP tools
claude chat --mcp

# Test pattern detection
./scripts/test-detection.js

# Development mode with hot reload
npm run dev
./scripts/register-dev.sh
```

## Expert Validation Status

**Multi-Expert Review Completed**: Amazon Q CLI, Rovodev, Kimi K2, Qwen3
- **Unanimous Agreement**: Prevention approach is correct
- **Key Finding**: Focus on 3 deep implementation patterns vs broad architectural detection
- **Critical Requirements**: Enhanced pattern matching, confidence uncertainty handling, production reliability

## Success Metrics (Expert-Validated)

- **30% reduction** in implementation-specific wrong suggestions
- **85%+ confidence** in critical pattern detection
- **95%+ adherence** to detected patterns in Claude suggestions
- **Context freshness** within 10 seconds of relevant changes

## Implementation Timeline (Proposed)

- **Phase 1**: Implementation Pattern Detection Engine (4 weeks)
- **Phase 2**: Proactive CLAUDE.md Maintenance (2 weeks)  
- **Phase 3**: Real-time System Hardening (2 weeks)
- **Total**: 8 weeks for production-ready system

## Working with This Project

Since this is a documentation-only project:

1. **For Analysis**: Read the three main documents to understand the architectural proposal
2. **For Research**: Focus on the expert validation findings and technical requirements
3. **For Implementation**: Use the detailed architectural specifications in CLAUDECAT-ARCHITECTURE.md
4. **For Context**: The project addresses real Claude Code accuracy problems through proactive awareness

## Key Innovation

**Core Problem**: Claude Code lacks project awareness at startup, leading to wrong implementation suggestions
**Solution**: Proactive implementation pattern detection that maintains CLAUDE.md with HOW patterns work, not just WHAT technologies exist

## Important Notes

- This is a fully implemented MCP server ready for production use
- Focus is on implementation details over technology detection
- Expert-validated approach with specific reliability requirements
- Prevention-focused: solve context problems before they happen