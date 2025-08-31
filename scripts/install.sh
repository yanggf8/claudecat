#!/bin/bash

# ClaudeCat MCP Server Installation Script
# Based on Cortex multi-instance approach

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CLAUDECAT_HOME="$HOME/.claudecat"

echo "🚀 Installing ClaudeCat MCP Server..."
echo "Project root: $PROJECT_ROOT"

# Create ClaudeCat home directory
mkdir -p "$CLAUDECAT_HOME/logs"

# Build the project
echo "📦 Building ClaudeCat..."
cd "$PROJECT_ROOT"
npm run build

# Check if build succeeded
if [ ! -f "$PROJECT_ROOT/dist/stdio-mcp-server.js" ]; then
    echo "❌ Build failed - stdio-mcp-server.js not found"
    exit 1
fi

# Make the server executable
chmod +x "$PROJECT_ROOT/dist/stdio-mcp-server.js"

# Create a symbolic link in ~/.local/bin if it exists
LOCAL_BIN="$HOME/.local/bin"
if [ -d "$LOCAL_BIN" ]; then
    ln -sf "$PROJECT_ROOT/dist/stdio-mcp-server.js" "$LOCAL_BIN/claudecat"
    echo "✅ Created symbolic link: ~/.local/bin/claudecat"
fi

# Register with Claude Code MCP system
echo "🔧 Registering ClaudeCat MCP server with Claude Code..."

# Remove any existing registration
claude mcp remove claudecat -s user 2>/dev/null || true
claude mcp remove claudecat -s local 2>/dev/null || true

# Add with full node path to avoid ENOENT errors
NODE_PATH=$(which node)
if [ -z "$NODE_PATH" ]; then
    echo "❌ Error: node command not found in PATH"
    exit 1
fi

echo "📍 Using node path: $NODE_PATH"
claude mcp add claudecat "$NODE_PATH" "$PROJECT_ROOT/dist/stdio-mcp-server.js"

echo "✅ Claude Code MCP server registered"

# Create session monitoring script
cat > "$CLAUDECAT_HOME/check-sessions.sh" << 'EOF'
#!/bin/bash
# ClaudeCat Multi-Instance Session Monitor

SESSIONS_FILE="$HOME/.claude/logs/claudecat"

if [ -d "$SESSIONS_FILE" ]; then
    echo "🔍 Active ClaudeCat Sessions:"
    echo "=============================="
    ls -la "$SESSIONS_FILE"/session-*.log 2>/dev/null | while read line; do
        echo "Session log: $(basename $(echo $line | awk '{print $9}'))"
        echo "Size: $(echo $line | awk '{print $5}') bytes"
        echo "Modified: $(echo $line | awk '{print $6, $7, $8}')"
        echo ""
    done
else
    echo "❌ No session logs directory found at $SESSIONS_FILE"
fi
EOF

chmod +x "$CLAUDECAT_HOME/check-sessions.sh"

# Create cleanup script
cat > "$CLAUDECAT_HOME/cleanup-sessions.sh" << 'EOF'
#!/bin/bash
# ClaudeCat Multi-Instance Session Cleanup

SESSIONS_FILE="$HOME/.claude/logs/claudecat"

if [ -f "$SESSIONS_FILE" ]; then
    echo "🧹 Cleaning up stale sessions..."
    
    # Create backup
    cp "$SESSIONS_FILE" "$SESSIONS_FILE.backup.$(date +%s)"
    
    # Remove sessions for non-existent processes
    jq 'with_entries(select(.value.pid as $pid | [$pid] | map(. as $p | ["ps", "-p", ($p | tostring)] | @sh) | .[0] as $cmd | ($cmd | split(" ") | .[0]) as $ps | ([$ps, "-p", ($p | tostring)] | @csv | gsub("\""; "") | . as $command | ($command | @sh) | . as $shellcmd | ($shellcmd | system) == 0)))' "$SESSIONS_FILE" > "$SESSIONS_FILE.tmp"
    
    mv "$SESSIONS_FILE.tmp" "$SESSIONS_FILE"
    echo "✅ Session cleanup complete"
else
    echo "❌ No active sessions file found"
fi
EOF

chmod +x "$CLAUDECAT_HOME/cleanup-sessions.sh"

echo ""
echo "✅ ClaudeCat MCP Server installation complete!"
echo ""
echo "📖 Usage Instructions:"
echo "1. Start Claude Code - it will automatically use the MCP server"
echo "2. Each Claude Code instance spawns its own ClaudeCat server instance"
echo "3. Monitor sessions with: ~/.claudecat/check-sessions.sh"
echo "4. Clean up stale sessions with: ~/.claudecat/cleanup-sessions.sh"
echo ""
echo "🔍 Troubleshooting:"
echo "- Check logs in: ~/.claudecat/logs/"
echo "- Session tracking: ~/.claudecat/logs/active-sessions.json"
echo "- Use 'session_analysis' tool in Claude Code to debug issues"
echo ""
echo "🎯 Ready for Claude Code usage!"