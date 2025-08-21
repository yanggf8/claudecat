#!/usr/bin/env node

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  Tool,
} from '@modelcontextprotocol/sdk/types.js';
import { ProactiveContextEngine } from './core/proactive-context-engine.js';

class ClaudeCatMCPServer {
  private server: Server;
  private contextEngine: ProactiveContextEngine;

  constructor() {
    this.server = new Server(
      {
        name: 'claudecat-proactive-context',
        version: '1.0.0',
      },
      {
        capabilities: {
          tools: {},
        },
      }
    );

    this.contextEngine = new ProactiveContextEngine();
    this.setupToolHandlers();
    this.setupEngineEventHandlers();
  }

  private setupToolHandlers() {
    // List available tools
    this.server.setRequestHandler(ListToolsRequestSchema, async () => {
      return {
        tools: [
          {
            name: 'get_project_context',
            description: 'Get current project context and implementation patterns',
            inputSchema: {
              type: 'object',
              properties: {},
              additionalProperties: false,
            },
          },
          {
            name: 'get_implementation_patterns',
            description: 'Get detected implementation patterns for auth, API responses, and error handling',
            inputSchema: {
              type: 'object',
              properties: {},
              additionalProperties: false,
            },
          },
          {
            name: 'get_critical_guardrails',
            description: 'Get critical guardrails based on detected project patterns',
            inputSchema: {
              type: 'object',
              properties: {},
              additionalProperties: false,
            },
          },
          {
            name: 'force_context_update',
            description: 'Force immediate re-detection of project patterns and CLAUDE.md update',
            inputSchema: {
              type: 'object',
              properties: {},
              additionalProperties: false,
            },
          },
          {
            name: 'get_engine_status',
            description: 'Get status information about the Cortex engine',
            inputSchema: {
              type: 'object',
              properties: {},
              additionalProperties: false,
            },
          },
        ] satisfies Tool[],
      };
    });

    // Handle tool calls
    this.server.setRequestHandler(CallToolRequestSchema, async (request) => {
      const { name, arguments: args } = request.params;

      try {
        switch (name) {
          case 'get_project_context':
            return await this.handleGetProjectContext();

          case 'get_implementation_patterns':
            return await this.handleGetImplementationPatterns();

          case 'get_critical_guardrails':
            return await this.handleGetCriticalGuardrails();

          case 'force_context_update':
            return await this.handleForceContextUpdate();

          case 'get_engine_status':
            return await this.handleGetEngineStatus();

          default:
            throw new Error(`Unknown tool: ${name}`);
        }
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : String(error);
        return {
          content: [
            {
              type: 'text',
              text: `Error executing ${name}: ${errorMessage}`,
            },
          ],
        };
      }
    });
  }

  private setupEngineEventHandlers() {
    this.contextEngine.on('context-updated', (context) => {
      console.log('📋 Project context updated:', {
        project: `${context.projectType} (${context.language})`,
        framework: context.framework,
        patterns: {
          auth: context.implementationPatterns.authentication.confidence,
          api: context.implementationPatterns.apiResponses.confidence,
          errors: context.implementationPatterns.errorHandling.confidence,
        },
      });
    });

    this.contextEngine.on('context-error', (error) => {
      console.error('❌ Context engine error:', error.message);
    });

    this.contextEngine.on('startup-complete', () => {
      console.log('🎯 ClaudeCat startup complete - Claude Code context ready');
    });
  }

  private async handleGetProjectContext() {
    const context = this.contextEngine.getCurrentContext();
    
    const summary = {
      projectType: context.projectType,
      language: context.language,
      framework: context.framework,
      packageManager: context.packageManager,
      lastUpdated: context.lastUpdated,
      keyDirectories: context.directories.map(d => `${d.path}/ (${d.purpose})`),
      coreDependencies: context.dependencies.slice(0, 10),
      scripts: context.scripts,
    };

    return {
      content: [
        {
          type: 'text',
          text: `**Project Context**

**Project**: ${summary.projectType}
**Language**: ${summary.language}  
**Framework**: ${summary.framework}
**Package Manager**: ${summary.packageManager}

**Key Directories**:
${summary.keyDirectories.map(d => `- ${d}`).join('\\n')}

**Core Dependencies**: ${summary.coreDependencies.join(', ')}

**Scripts**:
- dev: \`${summary.scripts.dev || 'Not detected'}\`
- build: \`${summary.scripts.build || 'Not detected'}\`
- test: \`${summary.scripts.test || 'Not detected'}\`

**Last Updated**: ${summary.lastUpdated}`,
        },
      ],
    };
  }

  private async handleGetImplementationPatterns() {
    const patterns = this.contextEngine.getImplementationPatternsSummary();
    const context = this.contextEngine.getCurrentContext();
    const impl = context.implementationPatterns;

    return {
      content: [
        {
          type: 'text',
          text: `**Implementation Patterns Detected**

**Authentication** (${impl.authentication.confidence}% confidence):
- User Property: \`${impl.authentication.userProperty}\`
- Token Storage: ${impl.authentication.tokenLocation}
- Error Response: ${impl.authentication.errorResponse.format}
- Middleware Pattern: ${impl.authentication.middlewarePattern}
${impl.authentication.evidence.length > 0 ? `- Evidence: ${impl.authentication.evidence.slice(0, 2).join(', ')}` : ''}

**API Responses** (${impl.apiResponses.confidence}% confidence):
- Success Format: ${impl.apiResponses.successFormat}
- Error Format: ${impl.apiResponses.errorFormat}
- Status Codes: ${impl.apiResponses.statusCodeUsage}
- Wrapper Pattern: ${impl.apiResponses.wrapperPattern}
${impl.apiResponses.evidence.length > 0 ? `- Evidence: ${impl.apiResponses.evidence.slice(0, 2).join(', ')}` : ''}

**Error Handling** (${impl.errorHandling.confidence}% confidence):
- Catch Pattern: ${impl.errorHandling.catchPattern}
- Error Structure: ${impl.errorHandling.errorStructure}
- Logging Integration: ${impl.errorHandling.loggingIntegration}
- Propagation Style: ${impl.errorHandling.propagationStyle}
${impl.errorHandling.evidence.length > 0 ? `- Evidence: ${impl.errorHandling.evidence.slice(0, 2).join(', ')}` : ''}

**Pattern Summary**: ${patterns.confidence}`,
        },
      ],
    };
  }

  private async handleGetCriticalGuardrails() {
    const guardrails = this.contextEngine.getCriticalGuardrails();

    if (guardrails.length === 0) {
      return {
        content: [
          {
            type: 'text',
            text: '**Critical Guardrails**\\n\\n⚠️ **Implementation patterns unclear** - Verify project conventions before making architectural decisions',
          },
        ],
      };
    }

    return {
      content: [
        {
          type: 'text',
          text: `**Critical Guardrails**

${guardrails.map((g, i) => `${i + 1}. ${g}`).join('\\n')}

*These guardrails are automatically generated based on detected implementation patterns with 60%+ confidence.*`,
        },
      ],
    };
  }

  private async handleForceContextUpdate() {
    const updatedContext = await this.contextEngine.forceUpdate();
    
    return {
      content: [
        {
          type: 'text',
          text: `**Context Update Complete**

Project patterns re-analyzed and CLAUDE.md updated.

**Updated**: ${updatedContext.lastUpdated}
**Confidence Levels**:
- Authentication: ${updatedContext.implementationPatterns.authentication.confidence}%
- API Responses: ${updatedContext.implementationPatterns.apiResponses.confidence}%
- Error Handling: ${updatedContext.implementationPatterns.errorHandling.confidence}%`,
        },
      ],
    };
  }

  private async handleGetEngineStatus() {
    const status = this.contextEngine.getStatus();
    
    return {
      content: [
        {
          type: 'text',
          text: `**ClaudeCat Engine Status**

**Initialization**: ${status.initialized ? '✅ Ready' : '❌ Not Ready'}
**Project Root**: ${status.projectRoot}

**File Watcher**:
- Status: ${status.watcher.isWatching ? '👀 Active' : '💤 Inactive'}
- Files Monitored: ${status.watcher.watchedFileCount}
- Processing Update: ${status.watcher.isProcessingUpdate ? '⏳ Yes' : '✅ No'}
- Last Update: ${status.watcher.lastUpdateTime?.toISOString() || 'Never'}`,
        },
      ],
    };
  }

  async start() {
    console.log('🚀 Starting ClaudeCat MCP Server...');
    
    // Initialize the context engine
    await this.contextEngine.initialize();
    
    // Start the MCP server with stdio transport
    const transport = new StdioServerTransport();
    await this.server.connect(transport);
    
    console.log('✅ ClaudeCat MCP Server ready - stdio transport active');
  }

  async stop() {
    console.log('🛑 Stopping ClaudeCat MCP Server...');
    await this.contextEngine.shutdown();
    console.log('✅ ClaudeCat MCP Server stopped');
  }
}

// Handle process signals for graceful shutdown
async function main() {
  const server = new ClaudeCatMCPServer();
  
  // Graceful shutdown handlers
  const shutdownHandler = async (signal: string) => {
    console.log(`\\n📡 Received ${signal}, shutting down gracefully...`);
    await server.stop();
    process.exit(0);
  };

  process.on('SIGINT', () => shutdownHandler('SIGINT'));
  process.on('SIGTERM', () => shutdownHandler('SIGTERM'));
  
  // Handle uncaught errors
  process.on('uncaughtException', (error) => {
    console.error('💥 Uncaught exception:', error);
    process.exit(1);
  });

  process.on('unhandledRejection', (reason) => {
    console.error('💥 Unhandled rejection:', reason);
    process.exit(1);
  });

  try {
    await server.start();
  } catch (error) {
    console.error('💥 Failed to start server:', error);
    process.exit(1);
  }
}

// Run the server if this file is executed directly
if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    console.error('💥 Fatal error:', error);
    process.exit(1);
  });
}