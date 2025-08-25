import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

export class MultiInstanceLogger {
  private logDir: string;
  private sessionId: string;
  private claudeSession: string;

  constructor() {
    this.logDir = path.join(os.homedir(), '.claudecat', 'multi-instance-logs');
    this.sessionId = this.generateSessionId();
    this.claudeSession = this.detectClaudeSession();
    
    // Create logs directory
    if (!fs.existsSync(this.logDir)) {
      fs.mkdirSync(this.logDir, { recursive: true });
    }
    
    this.logToFile(`SESSION_START: ${this.claudeSession} (PID: ${process.pid}, Session: ${this.sessionId})`);
    this.registerSession();
  }
  
  getSessionId(): string {
    return this.sessionId;
  }

  getClaudeSession(): string {
    return this.claudeSession;
  }

  private generateSessionId(): string {
    const timestamp = Date.now().toString(36);
    const random = Math.random().toString(36).substring(2, 8);
    return `claudecat-${timestamp}-${random}`;
  }
  
  private detectClaudeSession(): string {
    try {
      // Multiple detection methods for Claude Code instances
      const methods = [
        () => process.env.CLAUDE_SESSION_ID,
        () => process.env.CLAUDE_DESKTOP_SESSION,
        () => `claude-${process.ppid}`,
        () => `claude-time-${Date.now()}`
      ];
      
      for (const method of methods) {
        const result = method();
        if (result) {
          return result;
        }
      }
      
      return `unknown-claude-${Date.now()}`;
    } catch (error) {
      return `error-claude-${Date.now()}`;
    }
  }
  
  private registerSession(): void {
    try {
      const sessionsFile = path.join(this.logDir, 'active-sessions.json');
      let sessions: Record<string, any> = {};
      
      if (fs.existsSync(sessionsFile)) {
        const data = fs.readFileSync(sessionsFile, 'utf8');
        sessions = JSON.parse(data);
      }
      
      sessions[this.sessionId] = {
        sessionId: this.sessionId,
        claudeSession: this.claudeSession,
        pid: process.pid,
        parentPid: process.ppid,
        startTime: Date.now(),
        lastActivity: Date.now(),
        status: 'starting'
      };
      
      fs.writeFileSync(sessionsFile, JSON.stringify(sessions, null, 2));
      this.logToFile(`SESSION_REGISTERED: Total active sessions: ${Object.keys(sessions).length}`);
    } catch (error) {
      this.logToFile(`SESSION_REGISTER_ERROR: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  
  logToFile(message: string): void {
    try {
      const timestamp = new Date().toISOString();
      const logLine = `[${timestamp}] [${this.claudeSession}] [${this.sessionId}] ${message}\n`;
      const sessionLogFile = path.join(this.logDir, `${this.sessionId}.log`);
      fs.appendFileSync(sessionLogFile, logLine);
      console.error(`[Multi-Instance] ${logLine.trim()}`);
    } catch (error) {
      console.error(`[Multi-Instance] Failed to log: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  
  updateSessionStatus(status: string): void {
    try {
      const sessionsFile = path.join(this.logDir, 'active-sessions.json');
      if (fs.existsSync(sessionsFile)) {
        const data = fs.readFileSync(sessionsFile, 'utf8');
        const sessions = JSON.parse(data);
        
        if (sessions[this.sessionId]) {
          sessions[this.sessionId].status = status;
          sessions[this.sessionId].lastActivity = Date.now();
          fs.writeFileSync(sessionsFile, JSON.stringify(sessions, null, 2));
          this.logToFile(`SESSION_STATUS_UPDATED: ${status}`);
        }
      }
    } catch (error) {
      this.logToFile(`SESSION_STATUS_UPDATE_ERROR: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  getActiveSessionsInfo(): any {
    try {
      const sessionsFile = path.join(this.logDir, 'active-sessions.json');
      if (!fs.existsSync(sessionsFile)) {
        return { totalSessions: 0, sessions: [] };
      }
      
      const data = fs.readFileSync(sessionsFile, 'utf8');
      const sessions = JSON.parse(data);
      
      return {
        totalSessions: Object.keys(sessions).length,
        claudeSessions: [...new Set(Object.values(sessions).map((s: any) => s.claudeSession))],
        currentSession: this.sessionId,
        sessionDetails: Object.values(sessions).map((s: any) => ({
          sessionId: s.sessionId,
          claudeSession: s.claudeSession,
          pid: s.pid,
          uptime: Math.round((Date.now() - s.startTime) / 1000),
          status: s.status
        }))
      };
    } catch (error) {
      return { error: error instanceof Error ? error.message : String(error) };
    }
  }
  
  cleanup(): void {
    this.logToFile('SESSION_END: MCP server shutting down');
    
    try {
      const sessionsFile = path.join(this.logDir, 'active-sessions.json');
      if (fs.existsSync(sessionsFile)) {
        const data = fs.readFileSync(sessionsFile, 'utf8');
        const sessions = JSON.parse(data);
        delete sessions[this.sessionId];
        fs.writeFileSync(sessionsFile, JSON.stringify(sessions, null, 2));
        this.logToFile(`SESSION_CLEANUP: ${Object.keys(sessions).length} sessions remaining`);
      }
    } catch (error) {
      this.logToFile(`SESSION_CLEANUP_ERROR: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
}