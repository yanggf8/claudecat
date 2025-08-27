#!/bin/bash

# Cortex MCP Server Installation Script
set -e

echo "🚀 Installing ClaudeCat MCP Server..."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if Node.js is installed
if ! command -v node &> /dev/null; then
    print_error "Node.js is not installed. Please install Node.js 18+ first."
    exit 1
fi

# Check Node.js version
NODE_VERSION=$(node --version | cut -d'v' -f2 | cut -d'.' -f1)
if [ "$NODE_VERSION" -lt 18 ]; then
    print_error "Node.js version 18+ is required. Current version: $(node --version)"
    exit 1
fi

# Check if npm is installed
if ! command -v npm &> /dev/null; then
    print_error "npm is not installed. Please install npm first."
    exit 1
fi

# Check if Claude Code is installed
if ! command -v claude &> /dev/null; then
    print_warning "Claude Code CLI not found. Please install Claude Code first."
    print_status "Visit: https://docs.anthropic.com/en/docs/claude-code"
fi

# Install dependencies
print_status "Installing dependencies..."
npm install

# Build the project
print_status "Building TypeScript..."
npm run build

# Verify build output
if [ ! -f "dist/server.js" ]; then
    print_error "Build failed - dist/server.js not found"
    exit 1
fi

# Get absolute path
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
SERVER_PATH="$PROJECT_DIR/dist/server.js"

print_success "Build completed successfully"

# Register with Claude Code if available
if command -v claude &> /dev/null; then
    print_status "Registering with Claude Code..."
    
    # Remove existing registration if it exists
    claude mcp remove claudecat -s user 2>/dev/null || true
    claude mcp remove claudecat -s local 2>/dev/null || true
    
    # Get full node path to avoid ENOENT errors
    NODE_PATH=$(which node)
    if [ -z "$NODE_PATH" ]; then
        print_error "node command not found in PATH"
        exit 1
    fi
    
    print_status "Using node path: $NODE_PATH"
    
    # Add new registration with full node path
    if claude mcp add claudecat "$NODE_PATH" "$SERVER_PATH"; then
        print_success "MCP server registered with Claude Code"
        
        # Verify registration
        if claude mcp list | grep -q "claudecat"; then
            print_success "Registration verified"
        else
            print_warning "Registration may have failed - please verify with 'claude mcp list'"
        fi
    else
        print_error "Failed to register MCP server with Claude Code"
        exit 1
    fi
else
    print_warning "Claude Code not found - skipping registration"
    print_status "Manual registration command:"
    echo "  claude mcp add claudecat \"\$(which node)\" \"$SERVER_PATH\""
fi

# Create development registration script
print_status "Creating development scripts..."
cat > "$PROJECT_DIR/scripts/register-dev.sh" << EOF
#!/bin/bash
# Register development version with hot reload
claude mcp remove claudecat-dev -s user 2>/dev/null || true
claude mcp remove claudecat-dev -s local 2>/dev/null || true
claude mcp add claudecat-dev "\$(which node)" "\$(which tsx)" "$PROJECT_DIR/src/server.ts"
echo "Development server registered. Use 'npm run dev' to start with hot reload."
EOF

chmod +x "$PROJECT_DIR/scripts/register-dev.sh"

# Create uninstall script
cat > "$PROJECT_DIR/scripts/uninstall.sh" << EOF
#!/bin/bash
# Uninstall ClaudeCat MCP Server
claude mcp remove claudecat -s user 2>/dev/null || true
claude mcp remove claudecat -s local 2>/dev/null || true
claude mcp remove claudecat-dev -s user 2>/dev/null || true
claude mcp remove claudecat-dev -s local 2>/dev/null || true
echo "ClaudeCat MCP server unregistered from Claude Code"
EOF

chmod +x "$PROJECT_DIR/scripts/uninstall.sh"

print_success "Installation completed!"
print_status ""
print_status "Next steps:"
print_status "1. Start Claude Code with MCP: claude chat --mcp"
print_status "2. Use Cortex tools to get project context and patterns"
print_status ""
print_status "Available tools:"
print_status "- get_project_context: Get project information"
print_status "- get_implementation_patterns: Get detected patterns"
print_status "- get_critical_guardrails: Get project-specific rules"
print_status "- force_context_update: Force pattern re-detection"
print_status "- get_engine_status: Check engine status"
print_status ""
print_status "Development mode:"
print_status "- Run: npm run dev"
print_status "- Register dev server: ./scripts/register-dev.sh"
print_status ""
print_status "Uninstall:"
print_status "- Run: ./scripts/uninstall.sh"