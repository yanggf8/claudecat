#!/bin/bash

# ClaudeCat Multi-Instance MCP Server Installation Script
# Based on Cortex multi-instance approach

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CLAUDECAT_HOME="$HOME/.claudecat"

echo "🚀 Installing ClaudeCat Multi-Instance MCP Server..."
echo "Project root: $PROJECT_ROOT"

# Create ClaudeCat home directory
mkdir -p "$CLAUDECAT_HOME/multi-instance-logs"

# Build the project
echo "📦 Building ClaudeCat..."
cd "$PROJECT_ROOT"
npm run build

# Check if build succeeded
if [ ! -f "$PROJECT_ROOT/dist/multi-instance-server.js" ]; then
    echo "❌ Build failed - multi-instance-server.js not found"
    exit 1
fi

# Make the multi-instance server executable
chmod +x "$PROJECT_ROOT/dist/multi-instance-server.js"

# Create a symbolic link in ~/.local/bin if it exists
LOCAL_BIN="$HOME/.local/bin"
if [ -d "$LOCAL_BIN" ]; then
    ln -sf "$PROJECT_ROOT/dist/multi-instance-server.js" "$LOCAL_BIN/claudecat-multi"
    echo "✅ Created symbolic link: ~/.local/bin/claudecat-multi"
fi

# Update Claude Code MCP configuration
CLAUDE_CONFIG_DIR="$HOME/.config/claude-code"
CLAUDE_CONFIG="$CLAUDE_CONFIG_DIR/claude_desktop_config.json"

echo "🔧 Updating Claude Code MCP configuration..."

# Create config directory if it doesn't exist
mkdir -p "$CLAUDE_CONFIG_DIR"

# Create or update the config file
if [ -f "$CLAUDE_CONFIG" ]; then
    echo "📝 Backing up existing configuration..."
    cp "$CLAUDE_CONFIG" "$CLAUDE_CONFIG.backup.$(date +%s)"
fi

# Generate the new configuration
cat > "$CLAUDE_CONFIG" << EOF
{
  "mcpServers": {
    "claudecat": {
      "command": "node",
      "args": ["$PROJECT_ROOT/dist/multi-instance-server.js"],
      "env": {
        "CLAUDE_SESSION_ID": "\$\$CLAUDE_SESSION_ID"
      }
    }
  }
}
EOF

echo "✅ Claude Code MCP configuration updated"
echo "📁 Configuration file: $CLAUDE_CONFIG"

# Create session monitoring script
cat > "$CLAUDECAT_HOME/check-sessions.sh" << 'EOF'
#!/bin/bash
# ClaudeCat Multi-Instance Session Monitor

SESSIONS_FILE="$HOME/.claudecat/multi-instance-logs/active-sessions.json"

if [ -f "$SESSIONS_FILE" ]; then
    echo "🔍 Active ClaudeCat Sessions:"
    echo "=============================="
    cat "$SESSIONS_FILE" | jq -r '.[] | "Session: \(.sessionId)\nClaude: \(.claudeSession)\nPID: \(.pid)\nStatus: \(.status)\nUptime: \((now - (.startTime/1000)) / 60 | floor)m\n"'
else
    echo "❌ No active sessions file found"
fi
EOF

chmod +x "$CLAUDECAT_HOME/check-sessions.sh"

# Create cleanup script
cat > "$CLAUDECAT_HOME/cleanup-sessions.sh" << 'EOF'
#!/bin/bash
# ClaudeCat Multi-Instance Session Cleanup

SESSIONS_FILE="$HOME/.claudecat/multi-instance-logs/active-sessions.json"

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
echo "✅ ClaudeCat Multi-Instance MCP Server installation complete!"
echo ""
echo "📖 Usage Instructions:"
echo "1. Restart Claude Code completely to use the new multi-instance server"
echo "2. Each Claude Code instance will spawn its own ClaudeCat server"
echo "3. Monitor sessions with: ~/.claudecat/check-sessions.sh"
echo "4. Clean up stale sessions with: ~/.claudecat/cleanup-sessions.sh"
echo ""
echo "🔍 Troubleshooting:"
echo "- Check logs in: ~/.claudecat/multi-instance-logs/"
echo "- Session tracking: ~/.claudecat/multi-instance-logs/active-sessions.json"
echo "- Use 'session_analysis' tool in Claude Code to debug issues"
echo ""
echo "🎯 Ready for multi-instance Claude Code usage!"