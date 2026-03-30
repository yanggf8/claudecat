import { readFileSync, writeFileSync, existsSync, mkdirSync, chmodSync } from 'fs';
import { join, dirname } from 'path';
import { homedir } from 'os';

export interface CloudConfig {
  tursoUrl: string;
  tursoToken: string;
}

const CONFIG_PATH = join(homedir(), '.claudecat', 'config.json');

export class ConfigStore {
  static read(): CloudConfig | null {
    if (!existsSync(CONFIG_PATH)) return null;
    try {
      const raw = readFileSync(CONFIG_PATH, 'utf-8');
      const parsed = JSON.parse(raw);
      if (parsed.tursoUrl && parsed.tursoToken) {
        return { tursoUrl: parsed.tursoUrl, tursoToken: parsed.tursoToken };
      }
      return null;
    } catch {
      return null;
    }
  }

  static write(config: CloudConfig): void {
    const dir = dirname(CONFIG_PATH);
    mkdirSync(dir, { recursive: true });
    writeFileSync(CONFIG_PATH, JSON.stringify(config, null, 2) + '\n', 'utf-8');
    chmodSync(CONFIG_PATH, 0o600);
  }

  static exists(): boolean {
    return existsSync(CONFIG_PATH);
  }
}
